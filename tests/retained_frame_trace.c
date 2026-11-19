#define _GNU_SOURCE
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

/* Test-only deterministic poison of storage that XCreatePixmap leaves undefined.
 * Every sample is taken from the real X server before the real XCopyArea call.
 * No production timing or drawing code is replaced. */
struct Canvas { Display *display; Pixmap id; unsigned width, height, depth; };
static struct Canvas canvases[256];
static unsigned next_canvas;
static const unsigned long poison = 0xffc127daUL;
static int trace_fd = -2;
static int output(void) {
    if (trace_fd == -2) {
        const char *path = getenv("ZTTY_CANVAS_TRACE");
        trace_fd = path ? open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600) : -1;
    }
    return trace_fd;
}
Pixmap XCreatePixmap(Display *d, Drawable drawable, unsigned w, unsigned h, unsigned depth) {
    static Pixmap (*real)(Display *, Drawable, unsigned, unsigned, unsigned);
    if (!real) *(void **)(&real) = dlsym(RTLD_NEXT, "XCreatePixmap");
    Pixmap result = real(d, drawable, w, h, depth);
    if (output() >= 0 && depth == 32 && w > 1 && h > 1) {
        canvases[next_canvas++ % 256] = (struct Canvas){d,result,w,h,depth};
        const char *mode = getenv("ZTTY_CANVAS_POISON");
        if (!mode || mode[0] != '0') {
            GC gc = XCreateGC(d,result,0,NULL);
            XSetForeground(d,gc,poison);
            XFillRectangle(d,result,gc,0,0,w,h);
            XFreeGC(d,gc);
        }
        dprintf(trace_fd,"{\"kind\":\"create\",\"pixmap\":%lu,\"w\":%u,\"h\":%u}\n",result,w,h);
    }
    return result;
}
int XFreePixmap(Display *d, Pixmap p) {
    static int (*real)(Display *, Pixmap);
    if (!real) *(void **)(&real) = dlsym(RTLD_NEXT, "XFreePixmap");
    for (unsigned i=0;i<256;i++) if(canvases[i].display==d && canvases[i].id==p) canvases[i].id=0;
    return real(d,p);
}
int XCopyArea(Display *d, Drawable src, Drawable dst, GC gc, int sx, int sy,
              unsigned w, unsigned h, int dx, int dy) {
    static int (*real)(Display *, Drawable, Drawable, GC, int, int, unsigned, unsigned, int, int);
    if (!real) *(void **)(&real) = dlsym(RTLD_NEXT, "XCopyArea");
    for (unsigned i=0; i<256; i++) {
        struct Canvas *p = &canvases[i];
        if (p->id != src || p->display != d || output()<0) continue;
        if(sx>=0 && sy>=0 && w && h && (unsigned)sx+w<=p->width && (unsigned)sy+h<=p->height) {
            XImage *im = XGetImage(d,src,sx,sy,w,h,AllPlanes,ZPixmap);
            if (im) {
                unsigned long count=0,total=(unsigned long)w*h;
                for (unsigned y=0;y<h;y++) for(unsigned x=0;x<w;x++)
                    count += (XGetPixel(im,x,y)&0xffffffffUL)==poison;
                unsigned long first=XGetPixel(im,0,0),last=XGetPixel(im,w-1,h-1);
                dprintf(trace_fd,"{\"kind\":\"copy\",\"src\":%lu,\"dst\":%lu,\"w\":%u,\"h\":%u,\"poison_pixels\":%lu,\"pixels\":%lu,\"first\":%lu,\"last\":%lu}\n",
                        src,dst,w,h,count,total,first,last);
                XDestroyImage(im);
            }
        }
        break;
    }
    return real(d,src,dst,gc,sx,sy,w,h,dx,dy);
}
