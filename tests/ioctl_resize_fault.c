/* Test-only Linux ioctl fault injection. Never linked into ztty.
 *
 * Compile as a shared library, preload only into an owned test process tree,
 * and write "<broker pid>\n" to the private marker after initialization.
 * While the marker matches getpid(), TIOCSWINSZ reports a real EIO without
 * changing the PTY. Removing it restores normal kernel ioctl behavior.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <unistd.h>

static int marked_process(void) {
    const char *path = getenv("ZTTY_TEST_RESIZE_FAILURE_MARKER");
    if (!path) return 0;
    int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) return 0;
    char actual[64], expected[64];
    ssize_t bytes = read(fd, actual, sizeof actual);
    close(fd);
    int length = snprintf(expected, sizeof expected, "%ld\n", (long)getpid());
    return bytes == length && length > 0 && memcmp(actual, expected, (size_t)length) == 0;
}

int ioctl(int fd, unsigned long request, ...) {
    va_list args;
    va_start(args, request);
    void *argument = va_arg(args, void *);
    va_end(args);
    if (request == TIOCSWINSZ && marked_process()) {
        const struct winsize *size = argument;
        const char *path = getenv("ZTTY_TEST_RESIZE_FAILURE_LOG");
        if (path) {
            int output = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC | O_NOFOLLOW, 0600);
            if (output >= 0) {
                char line[192];
                int length = snprintf(line, sizeof line,
                    "{\"pid\":%ld,\"fd\":%d,\"rows\":%u,\"cols\":%u,\"errno\":%d}\n",
                    (long)getpid(), fd, (unsigned)size->ws_row, (unsigned)size->ws_col, EIO);
                if (length > 0 && (size_t)length < sizeof line) {
                    ssize_t written = write(output, line, (size_t)length);
                    (void)written; /* The Python gate verifies every required log record. */
                }
                close(output);
            }
        }
        errno = EIO;
        return -1;
    }
    return (int)syscall(SYS_ioctl, fd, request, argument);
}
