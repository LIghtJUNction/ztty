/* Seat-scoped clipboard and text-input services. Every stream is nonblocking,
 * bounded and independently owned; neither slow peers nor stale events can
 * stall the broker or inject a later transfer into a different request. */
#define _POSIX_C_SOURCE 200809L
#include "wayland_services.h"
#include "primary-selection-client-protocol.h"
#include "text-input-client-protocol.h"
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#define CLIP_LIMIT (4u * 1024u * 1024u)
#define SOURCE_BUDGET (8u * 1024u * 1024u)
#define STREAMS 16
#define IM_LIMIT 4096
static const char utf8_mime[] = "text/plain;charset=utf-8";
struct offer {
  struct offer *next;
  struct ztty_wl_services *s;
  void *proxy;
  unsigned primary;
  unsigned utf8, plain;
};
struct source {
  struct ztty_wl_services *s;
  void *proxy;
  unsigned primary, refs;
  uint8_t *bytes;
  size_t len;
};
struct outgoing {
  int fd;
  struct source *source;
  size_t offset;
  uint64_t deadline;
};
struct ztty_wl_services {
  struct wl_display *display;
  void *owner;
  ztty_wl_emit emit;
  struct wl_data_device_manager *manager;
  struct wl_data_device *device;
  struct zwp_primary_selection_device_manager_v1 *primary_manager;
  struct zwp_primary_selection_device_v1 *primary_device;
  struct zwp_text_input_manager_v3 *text_manager;
  struct zwp_text_input_v3 *text;
  struct wl_surface *surface;
  struct offer *offers, *selection[2];
  struct source *source[2];
  size_t source_bytes;
  struct outgoing outgoing[STREAMS];
  uint32_t serial, generation;
  int incoming_fd, incoming_error, complete;
  uint8_t *incoming;
  size_t incoming_len, incoming_cap, incoming_limit;
  uint64_t incoming_deadline;
  uint32_t im_serial, im_acked;
  int cursor_dirty;
  int im_entered, im_focused, im_enabled, im_invalid;
  char *pending_commit, *pending_preedit;
  char preedit[IM_LIMIT];
  int cursor[4];
};
static uint64_t now_ms(void) {
  struct timespec t;
  if (clock_gettime(CLOCK_MONOTONIC, &t))
    return 0;
  return (uint64_t)t.tv_sec * 1000 + (uint64_t)t.tv_nsec / 1000000;
}
static int private_nonblock(int fd) {
  int f = fcntl(fd, F_GETFL);
  int d = fcntl(fd, F_GETFD);
  return f < 0 || d < 0 || fcntl(fd, F_SETFL, f | O_NONBLOCK) < 0 ||
                 fcntl(fd, F_SETFD, d | FD_CLOEXEC) < 0
             ? -1
             : 0;
}
/* Suppress only this thread's new SIGPIPE while writing an owned offer FD.
 * No process-wide handler changes leak into shells or driver threads. */
static ssize_t stream_write(int fd, const uint8_t *data, size_t len) {
  sigset_t block, old, pending;
  sigemptyset(&block);
  sigaddset(&block, SIGPIPE);
  if (sigprocmask(SIG_BLOCK, &block, &old) < 0)
    return -1;
  sigpending(&pending);
  int was_pending = sigismember(&pending, SIGPIPE);
  ssize_t n = write(fd, data, len);
  int error = errno;
  if (n < 0 && error == EPIPE && !was_pending) {
    struct timespec zero = {0};
    while (sigtimedwait(&block, NULL, &zero) < 0 && errno == EINTR) {
    }
  }
  sigprocmask(SIG_SETMASK, &old, NULL);
  errno = error;
  return n;
}
static void source_unref(struct source *p) {
  if (!p || --p->refs)
    return;
  p->s->source_bytes -= p->len;
  free(p->bytes);
  free(p);
}
static void source_destroy(struct source *p) {
  if (!p)
    return;
  if (p->proxy) {
    if (p->primary)
      zwp_primary_selection_source_v1_destroy(p->proxy);
    else
      wl_data_source_destroy(p->proxy);
    p->proxy = NULL;
  }
  if (p->s->source[p->primary] == p)
    p->s->source[p->primary] = NULL;
  source_unref(p);
}
/* Payload accounting includes active selections and in-flight writers. When
 * both selections fill the budget, replacing an unshared selection may reclaim
 * that selection before allocating its replacement. A writer's snapshot is
 * immutable and is never reclaimed while its reference is live. */
static struct source *new_source(struct ztty_wl_services *s,
                                 const uint8_t *text, size_t len,
                                 unsigned primary) {
  if (len > CLIP_LIMIT)
    return NULL;
  struct source *old = s->source[primary];
  int reclaim = len > SOURCE_BUDGET - s->source_bytes;
  if (reclaim && (!old || old->refs != 1 ||
                  len > SOURCE_BUDGET - (s->source_bytes - old->len)))
    return NULL;
  struct source *p = calloc(1, sizeof(*p));
  if (!p)
    return NULL;
  if (reclaim)
    source_destroy(old);
  p->bytes = malloc(len ? len : 1);
  if (!p->bytes) {
    free(p);
    return NULL;
  }
  if (len)
    memcpy(p->bytes, text, len);
  p->len = len;
  p->refs = 1;
  p->s = s;
  p->primary = primary;
  s->source_bytes += len;
  return p;
}
static void send_source(struct source *p, const char *mime, int fd) {
  if (strcmp(mime, utf8_mime) && strcmp(mime, "text/plain") &&
      strcmp(mime, "UTF8_STRING")) {
    close(fd);
    return;
  }
  if (private_nonblock(fd) < 0) {
    close(fd);
    return;
  }
  for (unsigned i = 0; i < STREAMS; i++)
    if (p->s->outgoing[i].fd < 0) {
      p->refs++;
      p->s->outgoing[i] =
          (struct outgoing){.fd = fd, .source = p, .deadline = now_ms() + 2000};
      return;
    }
  close(fd);
}
static void data_target(void *d, struct wl_data_source *p, const char *mime) {
  (void)d;
  (void)p;
  (void)mime;
}
static void data_send(void *d, struct wl_data_source *p, const char *mime,
                      int fd) {
  (void)p;
  send_source(d, mime, fd);
}
static void data_cancel(void *d, struct wl_data_source *p) {
  (void)p;
  source_destroy(d);
}
static void data_drop(void *d, struct wl_data_source *p) {
  (void)d;
  (void)p;
}
static void data_action(void *d, struct wl_data_source *p, uint32_t a) {
  (void)d;
  (void)p;
  (void)a;
}
static const struct wl_data_source_listener source_listener = {
    .target = data_target,
    .send = data_send,
    .cancelled = data_cancel,
    .dnd_drop_performed = data_drop,
    .dnd_finished = data_drop,
    .action = data_action};
static void primary_send(void *d, struct zwp_primary_selection_source_v1 *p,
                         const char *mime, int fd) {
  (void)p;
  send_source(d, mime, fd);
}
static void primary_cancel(void *d, struct zwp_primary_selection_source_v1 *p) {
  (void)p;
  source_destroy(d);
}
static const struct zwp_primary_selection_source_v1_listener
    primary_source_listener = {.send = primary_send,
                               .cancelled = primary_cancel};
static void offer_mime(struct offer *o, const char *mime) {
  if (!strcmp(mime, utf8_mime))
    o->utf8 = 1;
  else if (!strcmp(mime, "text/plain"))
    o->plain = 1;
}
static void data_offer_mime(void *d, struct wl_data_offer *p, const char *m) {
  (void)p;
  offer_mime(d, m);
}
static void data_offer_action(void *d, struct wl_data_offer *p, uint32_t a) {
  (void)d;
  (void)p;
  (void)a;
}
static const struct wl_data_offer_listener offer_listener = {
    .offer = data_offer_mime,
    .source_actions = data_offer_action,
    .action = data_offer_action};
static void primary_offer_mime(void *d,
                               struct zwp_primary_selection_offer_v1 *p,
                               const char *m) {
  (void)p;
  offer_mime(d, m);
}
static const struct zwp_primary_selection_offer_v1_listener
    primary_offer_listener = {.offer = primary_offer_mime};
static void destroy_offer(struct offer *o) {
  if (o->primary)
    zwp_primary_selection_offer_v1_destroy(o->proxy);
  else
    wl_data_offer_destroy(o->proxy);
  free(o);
}
static void prune_offers(struct ztty_wl_services *s, int primary) {
  struct offer **p = &s->offers;
  while (*p) {
    struct offer *o = *p;
    if ((primary < 0 || (int)o->primary == primary) &&
        s->selection[o->primary] != o) {
      *p = o->next;
      destroy_offer(o);
    } else
      p = &o->next;
  }
}
static void new_offer(struct ztty_wl_services *s, void *proxy,
                      unsigned primary) {
  size_t count = 0;
  for (struct offer *o = s->offers; o; o = o->next)
    count++;
  struct offer *o = count < 8 ? calloc(1, sizeof(*o)) : NULL;
  if (!o) {
    if (primary)
      zwp_primary_selection_offer_v1_destroy(proxy);
    else
      wl_data_offer_destroy(proxy);
    return;
  }
  o->s = s;
  o->proxy = proxy;
  o->primary = primary;
  o->next = s->offers;
  s->offers = o;
  if (primary)
    zwp_primary_selection_offer_v1_add_listener(proxy, &primary_offer_listener,
                                                o);
  else
    wl_data_offer_add_listener(proxy, &offer_listener, o);
}
static void data_new_offer(void *d, struct wl_data_device *p,
                           struct wl_data_offer *o) {
  (void)p;
  new_offer(d, o, 0);
}
static void data_enter(void *d, struct wl_data_device *p, uint32_t serial,
                       struct wl_surface *surface, wl_fixed_t x, wl_fixed_t y,
                       struct wl_data_offer *o) {
  (void)p;
  (void)serial;
  (void)surface;
  (void)x;
  (void)y;
  (void)o;
  prune_offers(d, 0);
}
static void data_leave(void *d, struct wl_data_device *p) {
  (void)d;
  (void)p;
}
static void data_motion(void *d, struct wl_data_device *p, uint32_t t,
                        wl_fixed_t x, wl_fixed_t y) {
  (void)d;
  (void)p;
  (void)t;
  (void)x;
  (void)y;
}
static void data_selection(void *d, struct wl_data_device *p,
                           struct wl_data_offer *o) {
  (void)p;
  struct ztty_wl_services *s = d;
  s->selection[0] = o ? wl_data_offer_get_user_data(o) : NULL;
  prune_offers(s, 0);
}
static const struct wl_data_device_listener device_listener = {
    .data_offer = data_new_offer,
    .enter = data_enter,
    .leave = data_leave,
    .motion = data_motion,
    .drop = data_leave,
    .selection = data_selection};
static void primary_new_offer(void *d,
                              struct zwp_primary_selection_device_v1 *p,
                              struct zwp_primary_selection_offer_v1 *o) {
  (void)p;
  new_offer(d, o, 1);
}
static void primary_selection(void *d,
                              struct zwp_primary_selection_device_v1 *p,
                              struct zwp_primary_selection_offer_v1 *o) {
  (void)p;
  struct ztty_wl_services *s = d;
  s->selection[1] = o ? zwp_primary_selection_offer_v1_get_user_data(o) : NULL;
  prune_offers(s, 1);
}
static const struct zwp_primary_selection_device_v1_listener
    primary_device_listener = {.data_offer = primary_new_offer,
                               .selection = primary_selection};
static void clear_im_batch(struct ztty_wl_services *s) {
  free(s->pending_commit);
  free(s->pending_preedit);
  s->pending_commit = s->pending_preedit = NULL;
  s->im_invalid = 0;
}
static void im_commit_state(struct ztty_wl_services *s) {
  zwp_text_input_v3_commit(s->text);
  s->im_serial++;
}
static void im_commit_cursor(struct ztty_wl_services *s) {
  /* Cursor geometry is not an external edit. Some compositors retain the last
   * cause across commits instead of resetting it to INPUT_METHOD. */
  zwp_text_input_v3_set_text_change_cause(
      s->text, ZWP_TEXT_INPUT_V3_CHANGE_CAUSE_INPUT_METHOD);
  zwp_text_input_v3_set_cursor_rectangle(
      s->text, s->cursor[0], s->cursor[1], s->cursor[2], s->cursor[3]);
  im_commit_state(s);
}
static void im_sync(struct ztty_wl_services *s) {
  if (!s->text)
    return;
  int active = s->im_entered && s->im_focused;
  if (active && !s->im_enabled) {
    zwp_text_input_v3_enable(s->text);
    /* The terminal does not own the shell's edit buffer. Never invent a
     * surrounding document or translate arbitrary deletion to keystrokes. */
    zwp_text_input_v3_set_surrounding_text(s->text, "", 0, 0);
    zwp_text_input_v3_set_text_change_cause(
        s->text, ZWP_TEXT_INPUT_V3_CHANGE_CAUSE_OTHER);
    zwp_text_input_v3_set_content_type(
        s->text, ZWP_TEXT_INPUT_V3_CONTENT_HINT_NONE,
        ZWP_TEXT_INPUT_V3_CONTENT_PURPOSE_TERMINAL);
    zwp_text_input_v3_set_cursor_rectangle(s->text, s->cursor[0], s->cursor[1],
                                           s->cursor[2], s->cursor[3]);
    s->im_enabled = 1;
    im_commit_state(s);
  } else if (!active && s->im_enabled) {
    zwp_text_input_v3_disable(s->text);
    s->im_enabled = 0;
    im_commit_state(s);
    clear_im_batch(s);
    s->preedit[0] = 0;
    s->emit(s->owner, 10, 0, 0, NULL);
  }
}
static void im_enter(void *d, struct zwp_text_input_v3 *p,
                     struct wl_surface *surface) {
  (void)p;
  struct ztty_wl_services *s = d;
  s->im_entered = surface == s->surface;
  im_sync(s);
}
static void im_leave(void *d, struct zwp_text_input_v3 *p,
                     struct wl_surface *surface) {
  (void)p;
  (void)surface;
  struct ztty_wl_services *s = d;
  s->im_entered = 0;
  im_sync(s);
}
static void im_string(struct ztty_wl_services *s, char **destination,
                      const char *text) {
  size_t n = text ? strnlen(text, IM_LIMIT) : 0;
  free(*destination);
  *destination = NULL;
  if (n >= IM_LIMIT) {
    s->im_invalid = 1;
    return;
  }
  *destination = malloc(n + 1);
  if (!*destination) {
    s->im_invalid = 1;
    return;
  }
  if (n)
    memcpy(*destination, text, n);
  (*destination)[n] = 0;
}
static void im_preedit(void *d, struct zwp_text_input_v3 *p, const char *text,
                       int32_t begin, int32_t end) {
  (void)p;
  (void)begin;
  (void)end;
  struct ztty_wl_services *s = d;
  im_string(s, &s->pending_preedit, text);
}
static void im_commit(void *d, struct zwp_text_input_v3 *p, const char *text) {
  (void)p;
  struct ztty_wl_services *s = d;
  im_string(s, &s->pending_commit, text);
}
static void im_delete(void *d, struct zwp_text_input_v3 *p, uint32_t before,
                      uint32_t after) {
  (void)p;
  struct ztty_wl_services *s = d;
  if (before || after)
    s->im_invalid = 1;
}
static void im_done(void *d, struct zwp_text_input_v3 *p, uint32_t serial) {
  (void)p;
  struct ztty_wl_services *s = d;
  /* A stale done serial still commits text. The protocol only defers client
     state updates until a matching done; dropping text here loses input. */
  if (s->im_enabled && !s->im_invalid) {
    if (s->pending_commit) {
      const char *text = s->pending_commit;
      size_t n = strlen(text);
      while (n) {
        size_t take = n > 63 ? 63 : n;
        while (take && ((unsigned char)text[take] & 0xc0) == 0x80)
          take--;
        if (!take)
          break;
        char chunk[64];
        memcpy(chunk, text, take);
        chunk[take] = 0;
        s->emit(s->owner, 9, 0, 0, chunk);
        text += take;
        n -= take;
      }
    }
    const char *preedit = s->pending_preedit ? s->pending_preedit : "";
    memcpy(s->preedit, preedit, strlen(preedit) + 1);
    s->emit(s->owner, 10, 0, 0, NULL);
  }
  clear_im_batch(s);
  if (serial == s->im_serial) {
    s->im_acked = serial;
    if (s->im_enabled && s->cursor_dirty) {
      s->cursor_dirty = 0;
      im_commit_cursor(s);
    }
  }
}
static const struct zwp_text_input_v3_listener im_listener = {
    .enter = im_enter,
    .leave = im_leave,
    .preedit_string = im_preedit,
    .commit_string = im_commit,
    .delete_surrounding_text = im_delete,
    .done = im_done};
struct ztty_wl_services *ztty_wl_services_new(struct wl_display *display,
                                              void *owner, ztty_wl_emit emit) {
  struct ztty_wl_services *s = calloc(1, sizeof(*s));
  if (!s)
    return NULL;
  s->display = display;
  s->owner = owner;
  s->emit = emit;
  s->incoming_fd = -1;
  for (unsigned i = 0; i < STREAMS; i++)
    s->outgoing[i].fd = -1;
  s->cursor[2] = s->cursor[3] = 1;
  return s;
}
void ztty_wl_services_global(struct ztty_wl_services *s, struct wl_registry *r,
                             uint32_t name, const char *interface,
                             uint32_t version) {
  if (!strcmp(interface, wl_data_device_manager_interface.name) && !s->manager)
    s->manager = wl_registry_bind(r, name, &wl_data_device_manager_interface,
                                  version < 3 ? version : 3);
  else if (!strcmp(interface,
                   zwp_primary_selection_device_manager_v1_interface.name) &&
           !s->primary_manager)
    s->primary_manager = wl_registry_bind(
        r, name, &zwp_primary_selection_device_manager_v1_interface, 1);
  else if (!strcmp(interface, zwp_text_input_manager_v3_interface.name) &&
           !s->text_manager)
    s->text_manager =
        wl_registry_bind(r, name, &zwp_text_input_manager_v3_interface, 1);
}
void ztty_wl_services_start(struct ztty_wl_services *s, struct wl_seat *seat,
                            struct wl_surface *surface) {
  s->surface = surface;
  if (!seat)
    return;
  if (s->manager) {
    s->device = wl_data_device_manager_get_data_device(s->manager, seat);
    wl_data_device_add_listener(s->device, &device_listener, s);
  }
  if (s->primary_manager) {
    s->primary_device = zwp_primary_selection_device_manager_v1_get_device(
        s->primary_manager, seat);
    zwp_primary_selection_device_v1_add_listener(s->primary_device,
                                                 &primary_device_listener, s);
  }
  if (s->text_manager) {
    s->text = zwp_text_input_manager_v3_get_text_input(s->text_manager, seat);
    zwp_text_input_v3_add_listener(s->text, &im_listener, s);
  }
}
void ztty_wl_services_serial(struct ztty_wl_services *s, uint32_t serial) {
  s->serial = serial;
}
void ztty_wl_services_focus(struct ztty_wl_services *s, int focused) {
  s->im_focused = focused;
  im_sync(s);
}
void ztty_wl_services_cursor(struct ztty_wl_services *s, int x, int y,
                             int width, int height) {
  int rect[4] = {x, y, width, height};
  if (!memcmp(rect, s->cursor, sizeof(rect)))
    return;
  memcpy(s->cursor, rect, sizeof(rect));
  s->cursor_dirty = 1;
  if (s->text && s->im_enabled && s->im_acked == s->im_serial) {
    s->cursor_dirty = 0;
    im_commit_cursor(s);
  }
}
int ztty_wl_services_copy(struct ztty_wl_services *s, const uint8_t *text,
                          size_t len, int primary) {
  primary = !!primary;
  if (!s->serial || (!primary && !s->device) || (primary && !s->primary_device))
    return -1;
  struct source *p = new_source(s, text, len, (unsigned)primary);
  if (!p)
    return -1;
  p->proxy =
      primary ? (void *)zwp_primary_selection_device_manager_v1_create_source(
                    s->primary_manager)
              : (void *)wl_data_device_manager_create_data_source(s->manager);
  if (!p->proxy) {
    source_unref(p);
    return -1;
  }
  if (primary) {
    zwp_primary_selection_source_v1_add_listener(p->proxy,
                                                 &primary_source_listener, p);
    zwp_primary_selection_source_v1_offer(p->proxy, utf8_mime);
    zwp_primary_selection_source_v1_offer(p->proxy, "text/plain");
    zwp_primary_selection_device_v1_set_selection(s->primary_device, p->proxy,
                                                  s->serial);
  } else {
    wl_data_source_add_listener(p->proxy, &source_listener, p);
    wl_data_source_offer(p->proxy, utf8_mime);
    wl_data_source_offer(p->proxy, "text/plain");
    wl_data_device_set_selection(s->device, p->proxy, s->serial);
  }
  source_destroy(s->source[primary]);
  s->source[primary] = p;
  return 0;
}
int ztty_wl_services_available(struct ztty_wl_services *s, int primary) {
  struct offer *o = s->selection[!!primary];
  return o && (o->utf8 || o->plain);
}
void ztty_wl_services_cancel(struct ztty_wl_services *s) {
  if (s->incoming_fd >= 0)
    close(s->incoming_fd);
  s->incoming_fd = -1;
  free(s->incoming);
  s->incoming = NULL;
  s->incoming_len = s->incoming_cap = 0;
  s->complete = 0;
  s->incoming_error = 0;
  s->incoming_deadline = 0;
  s->generation = 0;
}
int ztty_wl_services_receive(struct ztty_wl_services *s, int primary,
                             size_t limit, uint32_t generation) {
  ztty_wl_services_cancel(s);
  struct offer *o = s->selection[!!primary];
  if (!o || (!o->utf8 && !o->plain) || !limit || limit > CLIP_LIMIT ||
      !generation)
    return -1;
  int fds[2];
  if (pipe(fds))
    return -1;
  if (private_nonblock(fds[0]) < 0 || fcntl(fds[1], F_SETFD, FD_CLOEXEC) < 0) {
    close(fds[0]);
    close(fds[1]);
    return -1;
  }
  const char *mime = o->utf8 ? utf8_mime : "text/plain";
  if (primary)
    zwp_primary_selection_offer_v1_receive(o->proxy, mime, fds[1]);
  else
    wl_data_offer_receive(o->proxy, mime, fds[1]);
  close(fds[1]);
  s->incoming_fd = fds[0];
  s->incoming_limit = limit;
  s->generation = generation;
  s->incoming_deadline = now_ms() + 2000;
  return 0;
}
static void finish_incoming(struct ztty_wl_services *s, int error) {
  if (s->incoming_fd >= 0)
    close(s->incoming_fd);
  s->incoming_fd = -1;
  s->incoming_deadline = 0;
  s->incoming_error = error;
  s->complete = 1;
  s->emit(s->owner, 8, s->generation, (uint32_t)error, NULL);
}
void ztty_wl_services_pump(struct ztty_wl_services *s) {
  uint64_t now = now_ms();
  size_t budget = 65536;
  for (unsigned i = 0; i < STREAMS && budget; i++) {
    struct outgoing *o = &s->outgoing[i];
    if (o->fd < 0)
      continue;
    int done = now >= o->deadline;
    size_t remaining = o->source->len - o->offset;
    if (!done && remaining) {
      ssize_t n = stream_write(o->fd, o->source->bytes + o->offset,
                               remaining < budget ? remaining : budget);
      if (n > 0) {
        o->offset += (size_t)n;
        budget -= (size_t)n;
      } else if (n < 0 && errno != EAGAIN && errno != EINTR)
        done = 1;
    }
    if (done || o->offset == o->source->len) {
      close(o->fd);
      o->fd = -1;
      source_unref(o->source);
      o->source = NULL;
    }
  }
  budget = 65536;
  while (s->incoming_fd >= 0 && budget) {
    if (now >= s->incoming_deadline) {
      finish_incoming(s, 1);
      break;
    }
    if (s->incoming_len == s->incoming_cap) {
      size_t cap = s->incoming_cap ? s->incoming_cap * 2 : 4096;
      if (cap > s->incoming_limit + 1)
        cap = s->incoming_limit + 1;
      uint8_t *next = realloc(s->incoming, cap);
      if (!next) {
        finish_incoming(s, 1);
        break;
      }
      s->incoming = next;
      s->incoming_cap = cap;
    }
    size_t available = s->incoming_cap - s->incoming_len;
    if (available > budget)
      available = budget;
    ssize_t n = read(s->incoming_fd, s->incoming + s->incoming_len, available);
    if (n > 0) {
      s->incoming_len += (size_t)n;
      budget -= (size_t)n;
      if (s->incoming_len > s->incoming_limit) {
        finish_incoming(s, 2);
        break;
      }
    } else if (n == 0) {
      finish_incoming(s, 0);
      break;
    } else {
      if (errno != EAGAIN && errno != EINTR)
        finish_incoming(s, 1);
      break;
    }
  }
}
size_t ztty_wl_services_fds(struct ztty_wl_services *s, struct pollfd *fds,
                            size_t cap) {
  size_t n = 0;
  if (s->incoming_fd >= 0 && n < cap)
    fds[n++] = (struct pollfd){.fd = s->incoming_fd, .events = POLLIN};
  for (unsigned i = 0; i < STREAMS && n < cap; i++)
    if (s->outgoing[i].fd >= 0)
      fds[n++] = (struct pollfd){.fd = s->outgoing[i].fd, .events = POLLOUT};
  return n;
}
uint64_t ztty_wl_services_deadline(struct ztty_wl_services *s) {
  uint64_t deadline = s->incoming_deadline;
  for (unsigned i = 0; i < STREAMS; i++)
    if (s->outgoing[i].fd >= 0 &&
        (!deadline || s->outgoing[i].deadline < deadline))
      deadline = s->outgoing[i].deadline;
  return deadline;
}
const uint8_t *ztty_wl_services_result(struct ztty_wl_services *s,
                                       uint32_t generation, size_t *len,
                                       int *error) {
  *len = 0;
  *error = 1;
  if (!s->complete || generation != s->generation)
    return NULL;
  *len = s->incoming_len;
  *error = s->incoming_error;
  return s->incoming;
}
const char *ztty_wl_services_preedit(struct ztty_wl_services *s) {
  return s->preedit;
}
void ztty_wl_services_destroy(struct ztty_wl_services *s) {
  if (!s)
    return;
  ztty_wl_services_cancel(s);
  clear_im_batch(s);
  for (unsigned i = 0; i < STREAMS; i++)
    if (s->outgoing[i].fd >= 0) {
      close(s->outgoing[i].fd);
      source_unref(s->outgoing[i].source);
    }
  source_destroy(s->source[0]);
  source_destroy(s->source[1]);
  s->selection[0] = s->selection[1] = NULL;
  prune_offers(s, -1);
  if (s->text)
    zwp_text_input_v3_destroy(s->text);
  if (s->text_manager)
    zwp_text_input_manager_v3_destroy(s->text_manager);
  if (s->primary_device)
    zwp_primary_selection_device_v1_destroy(s->primary_device);
  if (s->primary_manager)
    zwp_primary_selection_device_manager_v1_destroy(s->primary_manager);
  if (s->device) {
    if (wl_proxy_get_version((struct wl_proxy *)s->device) >= 2)
      wl_data_device_release(s->device);
    else
      wl_data_device_destroy(s->device);
  }
  if (s->manager)
    wl_data_device_manager_destroy(s->manager);
  free(s);
}
