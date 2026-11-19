/* Test the actual source allocator/transfer reference accounting without a
 * compositor. Generated Wayland protocol objects are linked normally. */
#include "../native/wayland_services.c"
#include <assert.h>
#include <stdio.h>
static struct source *install(struct ztty_wl_services *s, const uint8_t *text,
                              size_t len, unsigned primary) {
  struct source *p = new_source(s, text, len, primary);
  assert(p);
  source_destroy(s->source[primary]);
  s->source[primary] = p;
  assert(s->source_bytes <= SOURCE_BUDGET);
  return p;
}
int main(void) {
  uint8_t *full = malloc(CLIP_LIMIT);
  assert(full);
  memset(full, 'A', CLIP_LIMIT);
  struct ztty_wl_services s = {0};
  install(&s, full, CLIP_LIMIT, 0);
  struct source *primary = install(&s, full, CLIP_LIMIT, 1);
  assert(s.source_bytes == SOURCE_BUDGET);
  struct source *small = install(&s, (const uint8_t *)"b", 1, 0);
  assert(s.source_bytes == CLIP_LIMIT + 1);
  assert(s.source[1] == primary && primary->bytes[CLIP_LIMIT - 1] == 'A');
  assert(small->len == 1 && small->bytes[0] == 'b');
  assert(!new_source(&s, full, CLIP_LIMIT + 1, 1));
  assert(s.source[1] == primary && s.source_bytes == CLIP_LIMIT + 1);
  struct source *writer = install(&s, full, CLIP_LIMIT, 0);
  writer->refs++; /* Real outgoing streams hold this additional reference. */
  assert(!new_source(&s, (const uint8_t *)"c", 1, 0));
  assert(s.source[0] == writer && writer->refs == 2);
  assert(writer->bytes[0] == 'A' && s.source_bytes == SOURCE_BUDGET);
  source_unref(writer); /* Transfer finishes; its current selection remains. */
  install(&s, (const uint8_t *)"c", 1, 0);
  assert(s.source_bytes == CLIP_LIMIT + 1);
  writer = install(&s, full, CLIP_LIMIT, 0);
  writer->refs++;
  source_destroy(
      writer); /* Cancellation must not destroy a transfer snapshot. */
  assert(!s.source[0] && writer->refs == 1);
  assert(!new_source(&s, (const uint8_t *)"d", 1, 0));
  assert(writer->bytes[CLIP_LIMIT - 1] == 'A');
  source_unref(writer);
  install(&s, (const uint8_t *)"d", 1, 0);
  source_destroy(s.source[0]);
  source_destroy(s.source[1]);
  assert(s.source_bytes == 0);
  free(full);
  puts("PASS: full-budget replacement, oversize rejection, live-writer "
       "preservation, cancelled-writer lifetime, final accounting");
  return 0;
}
