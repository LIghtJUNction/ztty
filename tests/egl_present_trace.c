#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <time.h>

/* Test-only interposition. Production swap timing is unchanged. These calls
 * belong to the private fixture's single UI thread. The trace keeps the
 * existing XCopyArea fixture's eight-column geometry format. */
struct mapping { void *display; void *surface; uintptr_t window; };
static struct mapping surfaces[64];

void *eglCreateWindowSurface(void *display, void *config, uintptr_t window, const int *attributes) {
    static void *(*create)(void *, void *, uintptr_t, const int *);
    if (!create) *(void **)(&create) = dlsym(RTLD_NEXT, "eglCreateWindowSurface");
    if (!create) return NULL;
    void *surface = create(display, config, window, attributes);
    if (surface) {
        for (unsigned int i = 0; i < 64; ++i) {
            if (!surfaces[i].surface) {
                surfaces[i] = (struct mapping){display, surface, window};
                break;
            }
        }
    }
    return surface;
}

unsigned int eglDestroySurface(void *display, void *surface) {
    static unsigned int (*destroy)(void *, void *);
    if (!destroy) *(void **)(&destroy) = dlsym(RTLD_NEXT, "eglDestroySurface");
    if (!destroy) return 0;
    unsigned int result = destroy(display, surface);
    if (result) {
        for (unsigned int i = 0; i < 64; ++i) {
            if (surfaces[i].display == display && surfaces[i].surface == surface)
                surfaces[i] = (struct mapping){0};
        }
    }
    return result;
}

unsigned int eglSwapBuffers(void *display, void *surface) {
    static unsigned int (*swap)(void *, void *);
    static unsigned int (*query)(void *, void *, int, int *);
    if (!swap) *(void **)(&swap) = dlsym(RTLD_NEXT, "eglSwapBuffers");
    if (!query) *(void **)(&query) = dlsym(RTLD_NEXT, "eglQuerySurface");
    if (!swap || !query) return 0;
    /* Optional diagnosis records the actual back-buffer rows before a swap.
     * It does not alter the assertions or insert a settling delay. */
    const char *pixels_path = getenv("ZTTY_EGL_PIXEL_TRACE");
    unsigned char pixels[12] = {0};
    int sampled = 0, sample_width = 0, sample_height = 0;
    if (pixels_path && query(display, surface, 0x3057, &sample_width)
        && query(display, surface, 0x3056, &sample_height)) {
        static void (*read_pixels)(int, int, int, int, unsigned, unsigned, void *);
        if (!read_pixels) *(void **)(&read_pixels) = dlsym(RTLD_NEXT, "glReadPixels");
        if (read_pixels && sample_width > 32 && sample_height > 32) {
            int cw = (sample_width - 32) / 120, ch = (sample_height - 32) / 32;
            const int rows[3] = {0,16,31};
            for (int i=0;i<3;i++)
                read_pixels(16+118*cw+1,sample_height-(16+rows[i]*ch+1)-1,1,1,0x1908,0x1401,pixels+i*4);
            sampled=1;
        }
    }
    unsigned int result = swap(display, surface);
    const char *path = getenv("ZTTY_X11_PRESENT_TRACE");
    if (result && path) {
        uintptr_t window = 0;
        for (unsigned int i = 0; i < 64; ++i) {
            if (surfaces[i].display == display && surfaces[i].surface == surface) {
                window = surfaces[i].window;
                break;
            }
        }
        if (sampled && window) {
            int fd = open(pixels_path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
            if (fd >= 0) {
                struct timespec now;
                clock_gettime(CLOCK_MONOTONIC, &now);
                dprintf(fd,"%lu %lld %u %u %u %u %u %u %u %u %u %u %u %u\n",
                    (unsigned long)window,(long long)now.tv_sec*1000000000+now.tv_nsec,
                    pixels[0],pixels[1],pixels[2],pixels[3],pixels[4],pixels[5],
                    pixels[6],pixels[7],pixels[8],pixels[9],pixels[10],pixels[11]);
                close(fd);
            }
        }
        int width = 0, height = 0;
        if (window && query(display, surface, 0x3057, &width) && query(display, surface, 0x3056, &height)) {
            int fd = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
            if (fd >= 0) {
                char line[256];
                int length = snprintf(line, sizeof line, "%lu %lu 0 0 %d %d 0 0\n",
                    (unsigned long)(uintptr_t)surface, (unsigned long)window, width, height);
                if (length > 0 && (unsigned int)length < sizeof line)
                    (void)write(fd, line, (size_t)length);
                close(fd);
            }
        }
    }
    return result;
}
