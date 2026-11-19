#ifndef ZTTY_WAYLAND_SERVICES_H
#define ZTTY_WAYLAND_SERVICES_H
#include <poll.h>
#include <stddef.h>
#include <stdint.h>
#include <wayland-client.h>
struct ztty_wl_services;
typedef void (*ztty_wl_emit)(void *, uint32_t, uint32_t, uint32_t,
                             const char *);
struct ztty_wl_services *ztty_wl_services_new(struct wl_display *, void *,
                                              ztty_wl_emit);
void ztty_wl_services_destroy(struct ztty_wl_services *);
void ztty_wl_services_global(struct ztty_wl_services *, struct wl_registry *,
                             uint32_t, const char *, uint32_t);
void ztty_wl_services_start(struct ztty_wl_services *, struct wl_seat *,
                            struct wl_surface *);
void ztty_wl_services_serial(struct ztty_wl_services *, uint32_t);
void ztty_wl_services_focus(struct ztty_wl_services *, int);
void ztty_wl_services_cursor(struct ztty_wl_services *, int, int, int, int);
void ztty_wl_services_pump(struct ztty_wl_services *);
size_t ztty_wl_services_fds(struct ztty_wl_services *, struct pollfd *, size_t);
uint64_t ztty_wl_services_deadline(struct ztty_wl_services *);
int ztty_wl_services_copy(struct ztty_wl_services *, const uint8_t *, size_t,
                          int);
int ztty_wl_services_available(struct ztty_wl_services *, int);
int ztty_wl_services_receive(struct ztty_wl_services *, int, size_t, uint32_t);
void ztty_wl_services_cancel(struct ztty_wl_services *);
const uint8_t *ztty_wl_services_result(struct ztty_wl_services *, uint32_t,
                                       size_t *, int *);
const char *ztty_wl_services_preedit(struct ztty_wl_services *);
#endif
