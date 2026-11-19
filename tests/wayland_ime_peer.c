/* Private-compositor test peer, never connected to the host desktop.
 * Protocol XML: swaywm/wlroots 0855cdacb2eeeff35849e2e9c4db0aa996d78d10.
 * This is an actual input-method-v2 client exercising text-input-v3 in ztty. */
#define _POSIX_C_SOURCE 200809L
#include "input-method-client-protocol.h"
#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <wayland-client.h>
struct state {
  struct wl_display *display;
  struct wl_seat *seat;
  struct zwp_input_method_manager_v2 *manager;
  struct zwp_input_method_v2 *im;
  uint32_t serial, purpose, hint;
  int active, failed;
  size_t surrounding;
};
static void activate(void *d, struct zwp_input_method_v2 *im) {
  (void)im;
  struct state *s = d;
  s->active = 1;
}
static void deactivate(void *d, struct zwp_input_method_v2 *im) {
  (void)im;
  struct state *s = d;
  s->active = 0;
}
static void surrounding(void *d, struct zwp_input_method_v2 *im,
                        const char *text, uint32_t cursor, uint32_t anchor) {
  (void)im;
  (void)cursor;
  (void)anchor;
  struct state *s = d;
  s->surrounding = strlen(text);
}
static void cause(void *d, struct zwp_input_method_v2 *im, uint32_t cause) {
  (void)d;
  (void)im;
  (void)cause;
}
static void content(void *d, struct zwp_input_method_v2 *im, uint32_t hint,
                    uint32_t purpose) {
  (void)im;
  struct state *s = d;
  s->hint = hint;
  s->purpose = purpose;
}
static void done(void *d, struct zwp_input_method_v2 *im) {
  (void)im;
  struct state *s = d;
  s->serial++;
  printf("STATE %d %u %u %zu\n", s->active, s->serial, s->purpose,
         s->surrounding);
  fflush(stdout);
}
static void unavailable(void *d, struct zwp_input_method_v2 *im) {
  (void)im;
  struct state *s = d;
  s->failed = 1;
  puts("UNAVAILABLE");
  fflush(stdout);
}
static const struct zwp_input_method_v2_listener listener = {
    .activate = activate,
    .deactivate = deactivate,
    .surrounding_text = surrounding,
    .text_change_cause = cause,
    .content_type = content,
    .done = done,
    .unavailable = unavailable};
static void global(void *d, struct wl_registry *r, uint32_t name,
                   const char *interface, uint32_t version) {
  struct state *s = d;
  if (!strcmp(interface, wl_seat_interface.name) && !s->seat)
    s->seat = wl_registry_bind(r, name, &wl_seat_interface,
                               version < 5 ? version : 5);
  else if (!strcmp(interface, zwp_input_method_manager_v2_interface.name) &&
           !s->manager)
    s->manager =
        wl_registry_bind(r, name, &zwp_input_method_manager_v2_interface, 1);
}
static void removed(void *d, struct wl_registry *r, uint32_t name) {
  (void)d;
  (void)r;
  (void)name;
}
static const struct wl_registry_listener globals = {.global = global,
                                                    .global_remove = removed};
static int command(struct state *s, char *line) {
  size_t len = strlen(line);
  if (len && line[len - 1] == '\n')
    line[--len] = 0;
  if (!strcmp(line, "quit"))
    return 0;
  if (!strncmp(line, "preedit ", 8))
    zwp_input_method_v2_set_preedit_string(
        s->im, line + 8, (int32_t)strlen(line + 8), (int32_t)strlen(line + 8));
  else if (!strncmp(line, "commit ", 7)) {
    zwp_input_method_v2_commit_string(s->im, line + 7);
    zwp_input_method_v2_set_preedit_string(s->im, "", 0, 0);
  } else if (!strcmp(line, "delete")) {
    zwp_input_method_v2_delete_surrounding_text(s->im, 1, 0);
    zwp_input_method_v2_commit_string(s->im, "MUST_NOT_INSERT");
  } else if (!strcmp(line, "status")) {
    printf("STATE %d %u %u %zu\n", s->active, s->serial, s->purpose,
           s->surrounding);
    fflush(stdout);
    return 1;
  } else {
    puts("INVALID");
    fflush(stdout);
    return 1;
  }
  zwp_input_method_v2_commit(s->im, s->serial);
  wl_display_flush(s->display);
  printf("SENT %u\n", s->serial);
  fflush(stdout);
  return 1;
}
int main(void) {
  struct state s = {0};
  s.display = wl_display_connect(NULL);
  if (!s.display)
    return 2;
  struct wl_registry *r = wl_display_get_registry(s.display);
  wl_registry_add_listener(r, &globals, &s);
  if (wl_display_roundtrip(s.display) < 0 || !s.seat || !s.manager)
    return 3;
  s.im = zwp_input_method_manager_v2_get_input_method(s.manager, s.seat);
  zwp_input_method_v2_add_listener(s.im, &listener, &s);
  wl_display_flush(s.display);
  int running = 1;
  char line[8192];
  size_t used = 0;
  while (running && !s.failed) {
    while (wl_display_prepare_read(s.display) != 0)
      if (wl_display_dispatch_pending(s.display) < 0) {
        s.failed = 1;
        break;
      }
    if (s.failed)
      break;
    if (wl_display_flush(s.display) < 0 && errno != EAGAIN) {
      wl_display_cancel_read(s.display);
      break;
    }
    struct pollfd fds[2] = {
        {.fd = wl_display_get_fd(s.display), .events = POLLIN},
        {.fd = 0, .events = POLLIN}};
    int ready = poll(fds, 2, 10000);
    if (ready <= 0) {
      wl_display_cancel_read(s.display);
      if (ready < 0 && errno == EINTR)
        continue;
      break;
    }
    if (fds[0].revents & POLLIN) {
      if (wl_display_read_events(s.display) < 0)
        break;
    } else
      wl_display_cancel_read(s.display);
    if (wl_display_dispatch_pending(s.display) < 0)
      break;
    if (fds[1].revents & (POLLIN | POLLHUP)) {
      char byte;
      ssize_t n = read(0, &byte, 1);
      if (n <= 0)
        break;
      if (used + 1 == sizeof(line)) {
        s.failed = 1;
        break;
      }
      line[used++] = byte;
      if (byte == '\n') {
        line[used] = 0;
        running = command(&s, line);
        used = 0;
      }
    }
  }
  zwp_input_method_v2_destroy(s.im);
  zwp_input_method_manager_v2_destroy(s.manager);
  wl_seat_release(s.seat);
  wl_registry_destroy(r);
  wl_display_disconnect(s.display);
  return s.failed ? 1 : 0;
}
