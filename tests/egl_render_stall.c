#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

/* Only the owned test loads this shim. Arming a private file pauses exactly the
 * presentation boundary; removing it releases the baseline without replaying
 * any PTY input. A new isolated helper is instead retired by its deadline. */
unsigned int eglSwapBuffers(void *display, void *surface) {
    static unsigned int (*real_swap)(void *, void *);
    if (!real_swap)
        *(void **)(&real_swap) = dlsym(RTLD_NEXT, "eglSwapBuffers");
    const char *trigger = getenv("ZTTY_TEST_RENDER_STALL");
    if (trigger && access(trigger, F_OK) == 0) {
        char marker[4096];
        int size = snprintf(marker, sizeof(marker), "%s.entered-%ld", trigger, (long)getpid());
        if (size > 0 && (size_t)size < sizeof(marker)) {
            int fd = open(marker, O_WRONLY | O_CREAT | O_CLOEXEC, 0600);
            if (fd >= 0) close(fd);
        }
        while (access(trigger, F_OK) == 0) poll(NULL, 0, 5);
    }
    return real_swap ? real_swap(display, surface) : 0;
}
