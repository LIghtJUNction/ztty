#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

/* Test-only interposition: record actual visible-copy calls, without changing
 * their geometry, ordering, or flushing. The production binary is unmodified. */
int XCopyArea(void *display, unsigned long source, unsigned long destination,
              void *gc, int x, int y, unsigned int width, unsigned int height,
              int destination_x, int destination_y) {
    static int (*copy)(void *, unsigned long, unsigned long, void *, int, int,
                       unsigned int, unsigned int, int, int);
    if (!copy)
        *(void **)(&copy) = dlsym(RTLD_NEXT, "XCopyArea");
    int result = copy(display, source, destination, gc, x, y, width, height,
                      destination_x, destination_y);
    const char *path = getenv("ZTTY_X11_PRESENT_TRACE");
    if (path) {
        int fd = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
        if (fd >= 0) {
            char line[256];
            int length = snprintf(line, sizeof line,
                "%lu %lu %d %d %u %u %d %d\n",
                source, destination, x, y, width, height,
                destination_x, destination_y);
            if (length > 0 && (unsigned int)length < sizeof line)
                (void)write(fd, line, (size_t)length);
            close(fd);
        }
    }
    return result;
}
