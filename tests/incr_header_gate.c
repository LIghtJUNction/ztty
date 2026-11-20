/* Test-only scheduling gate. Load only in a private-Xvfb fixture's broker.
 * It preserves every Xlib call and pauses a successful delete-on-read INCR
 * header until the fixture owner has synchronously installed its first chunk.
 * No payload bytes, user properties, or production configuration are written.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* Xlib's public ABI; the opaque display is never dereferenced. Keeping these
 * declarations local avoids requiring X11 development headers for the fixture.
 */
typedef struct _XDisplay Display;
typedef unsigned long Window;
typedef unsigned long Atom;
typedef int Bool;
enum { None = 0, AnyPropertyType = 0, Success = 0, BadImplementation = 17 };

static Display *armed_display;
static Window armed_window;
static Atom armed_property;
static int armed;

static void notify(const char *event, Window window, Atom property,
                   Atom type, unsigned long count, int code) {
    const char *path = getenv("ZTTY_INCR_TEST_EVENTS");
    if (!path || !*path) return;
    int fd = open(path, O_WRONLY | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) return;
    char line[384];
    int n = snprintf(line, sizeof(line),
        "{\"event\":\"%s\",\"pid\":%ld,\"window\":%lu,\"property\":%lu,"
        "\"type\":%lu,\"count\":%lu,\"code\":%d}\n",
        event, (long)getpid(), window, property, type, count, code);
    if (n > 0 && (size_t)n < sizeof(line)) {
        ssize_t sent;
        do { sent = write(fd, line, (size_t)n); } while (sent < 0 && errno == EINTR);
    }
    close(fd);
}

static int release_header(void) {
    const char *path = getenv("ZTTY_INCR_TEST_RELEASE");
    if (!path || !*path) return 0;
    int fd = open(path, O_RDONLY | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) return 0;
    struct pollfd wait = { .fd = fd, .events = POLLIN, .revents = 0 };
    int ready;
    do { ready = poll(&wait, 1, 10000); } while (ready < 0 && errno == EINTR);
    char byte = 0;
    ssize_t got = ready > 0 ? read(fd, &byte, 1) : -1;
    close(fd);
    return got == 1 && byte == 'R';
}

int XGetWindowProperty(Display *display, Window window, Atom property,
                       long offset, long length, Bool delete, Atom requested,
                       Atom *actual, int *format, unsigned long *count,
                       unsigned long *remaining, unsigned char **data) {
    typedef int (*Get)(Display *, Window, Atom, long, long, Bool, Atom,
                       Atom *, int *, unsigned long *, unsigned long *, unsigned char **);
    Get next = (Get)dlsym(RTLD_NEXT, "XGetWindowProperty");
    if (!next) return BadImplementation;
    int first_data = armed && display == armed_display
        && window == armed_window && property == armed_property;
    int code = next(display, window, property, offset, length, delete, requested,
                    actual, format, count, remaining, data);
    if (first_data) {
        armed = 0;
        notify("first_data_read", window, property, *actual, *count, code);
    }
    const char *atom = getenv("ZTTY_INCR_TEST_ATOM");
    Atom incr = atom ? strtoul(atom, NULL, 10) : None;
    if (code == Success && incr != None && delete && requested == AnyPropertyType
        && *actual == incr && *format == 32 && *count == 1 && *remaining == 0) {
        notify("header_deleted_by_get", window, property, *actual, *count, code);
        if (!release_header()) {
            notify("gate_failed", window, property, *actual, *count, code);
            return code;
        }
        armed_display = display;
        armed_window = window;
        armed_property = property;
        armed = 1;
        notify("header_returned", window, property, *actual, *count, code);
    }
    return code;
}

int XDeleteProperty(Display *display, Window window, Atom property) {
    typedef int (*Delete)(Display *, Window, Atom);
    Delete next = (Delete)dlsym(RTLD_NEXT, "XDeleteProperty");
    if (!next) return BadImplementation;
    if (armed && display == armed_display && window == armed_window && property == armed_property)
        notify("delete_before_first_data_read", window, property, None, 0, 0);
    return next(display, window, property);
}
