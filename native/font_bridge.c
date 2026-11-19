/* Small ABI boundary for the system FreeType, Fontconfig and HarfBuzz APIs.
 * Rendering and terminal state remain in Rust. No display connection is used. */
#include <fontconfig/fontconfig.h>
#include <ft2build.h>
#include FT_FREETYPE_H
#include FT_SYNTHESIS_H
#include <hb.h>
#include <hb-ft.h>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define FONT_LIMIT 64
#define TEXT_LIMIT 16384

typedef struct {
    float cell_width, cell_height, ascent;
} ZttyFontMetrics;

typedef struct {
    uint32_t glyph, cluster;
    float x_advance, y_advance, x_offset, y_offset;
} ZttyShapedGlyph;

typedef struct {
    const unsigned char *pixels;
    int width, height, pitch, format, left, top;
    float scale;
} ZttyGlyphBitmap;

typedef struct {
    FT_Face face;
    hb_font_t *hb;
    char *file;
    int index;
    float scale;
    unsigned style;
} FontFace;

typedef struct {
    FT_Library library;
    FcPattern *base;
    FontFace faces[FONT_LIMIT];
    unsigned count;
    double pixels;
} ZttyFonts;

static uint32_t next_scalar(const unsigned char *s, size_t n, size_t *offset) {
    unsigned c = s[(*offset)++];
    if (c < 128) return c;
    unsigned count = c < 224 ? 1 : c < 240 ? 2 : 3;
    uint32_t value = c & (count == 1 ? 31 : count == 2 ? 15 : 7);
    while (count-- && *offset < n) value = (value << 6) | (s[(*offset)++] & 63);
    return value;
}

static int open_match(ZttyFonts *fonts, FcPattern *match, unsigned style) {
    FcChar8 *file = NULL;
    int index = 0;
    if (FcPatternGetString(match, FC_FILE, 0, &file) != FcResultMatch ||
        !file || strlen((const char *)file) > 4096) return -1;
    FcPatternGetInteger(match, FC_INDEX, 0, &index);
    for (unsigned i = 0; i < fonts->count; ++i) {
        FontFace *entry = &fonts->faces[i];
        if (entry->index == index && entry->style == style && strcmp(entry->file, (const char *)file) == 0)
            return (int)i;
    }
    if (fonts->count == FONT_LIMIT) return -1;
    FT_Face face = NULL;
    if (FT_New_Face(fonts->library, (const char *)file, index, &face)) return -1;
    float scale = 1.0f;
    FT_Error error = FT_Set_Char_Size(face, 0, (FT_F26Dot6)lround(fonts->pixels * 64), 72, 72);
    if (error && face->num_fixed_sizes > 0) {
        int best = 0;
        double distance = 1e30;
        for (int i = 0; i < face->num_fixed_sizes; ++i) {
            double size = face->available_sizes[i].y_ppem / 64.0;
            double candidate = fabs(size - fonts->pixels);
            if (candidate < distance) { distance = candidate; best = i; }
        }
        error = FT_Select_Size(face, best);
        double size = face->available_sizes[best].y_ppem / 64.0;
        if (size > 0) scale = (float)(fonts->pixels / size);
    }
    if (error || !face->size) { FT_Done_Face(face); return -1; }
    hb_font_t *hb = hb_ft_font_create_referenced(face);
    if (!hb) { FT_Done_Face(face); return -1; }
    hb_ft_font_set_load_flags(hb, FT_LOAD_DEFAULT | FT_LOAD_COLOR);
    char *owned = malloc(strlen((const char *)file) + 1);
    if (!owned) { hb_font_destroy(hb); FT_Done_Face(face); return -1; }
    strcpy(owned, (const char *)file);
    unsigned id = fonts->count++;
    fonts->faces[id] = (FontFace){face, hb, owned, index, scale, style};
    return (int)id;
}

int ztty_fonts_select(ZttyFonts *fonts, const unsigned char *utf8, size_t length,
                      unsigned style) {
    if (!fonts || length > TEXT_LIMIT || (length && !utf8)) return -1;
    FcPattern *pattern = FcPatternDuplicate(fonts->base);
    FcCharSet *charset = FcCharSetCreate();
    if (!pattern || !charset) {
        if (pattern) FcPatternDestroy(pattern);
        if (charset) FcCharSetDestroy(charset);
        return -1;
    }
    size_t offset = 0;
    while (offset < length) {
        uint32_t scalar = next_scalar(utf8, length, &offset);
        /* Joiners, selectors and tag characters need not have standalone glyphs. */
        if (scalar != 0x200c && scalar != 0x200d &&
            !(scalar >= 0xfe00 && scalar <= 0xfe0f) &&
            !(scalar >= 0xe0000 && scalar <= 0xe01ef))
            FcCharSetAddChar(charset, scalar);
    }
    FcPatternDel(pattern, FC_CHARSET);
    FcPatternAddCharSet(pattern, FC_CHARSET, charset);
    FcCharSetDestroy(charset);
    FcPatternDel(pattern, FC_WEIGHT);
    FcPatternAddInteger(pattern, FC_WEIGHT, style & 1 ? FC_WEIGHT_BOLD : FC_WEIGHT_REGULAR);
    FcPatternDel(pattern, FC_SLANT);
    FcPatternAddInteger(pattern, FC_SLANT, style & 2 ? FC_SLANT_ITALIC : FC_SLANT_ROMAN);
    FcPatternDel(pattern, FC_PIXEL_SIZE);
    FcPatternAddDouble(pattern, FC_PIXEL_SIZE, fonts->pixels);
    FcConfigSubstitute(NULL, pattern, FcMatchPattern);
    FcDefaultSubstitute(pattern);
    FcResult result;
    FcPattern *match = FcFontMatch(NULL, pattern, &result);
    FcPatternDestroy(pattern);
    if (!match) return -1;
    int id = open_match(fonts, match, style);
    FcPatternDestroy(match);
    return id;
}

void ztty_fonts_free(ZttyFonts *fonts) {
    if (!fonts) return;
    for (unsigned i = 0; i < fonts->count; ++i) {
        hb_font_destroy(fonts->faces[i].hb);
        FT_Done_Face(fonts->faces[i].face);
        free(fonts->faces[i].file);
    }
    if (fonts->base) FcPatternDestroy(fonts->base);
    if (fonts->library) FT_Done_FreeType(fonts->library);
    free(fonts);
}

ZttyFonts *ztty_fonts_new(const char *description, ZttyFontMetrics *metrics) {
    if (!description || !metrics || strlen(description) > 4096 || !FcInit()) return NULL;
    ZttyFonts *fonts = calloc(1, sizeof(*fonts));
    if (!fonts) return NULL;
    fonts->base = FcNameParse((const FcChar8 *)description);
    if (!fonts->base || FT_Init_FreeType(&fonts->library)) {
        ztty_fonts_free(fonts); return NULL;
    }
    /* Keep the original family pattern. Applying substitution twice can
     * prepend fallback families twice and select a color emoji face for ASCII. */
    FcPattern *defaults = FcPatternDuplicate(fonts->base);
    if (!defaults) { ztty_fonts_free(fonts); return NULL; }
    FcConfigSubstitute(NULL, defaults, FcMatchPattern);
    FcDefaultSubstitute(defaults);
    fonts->pixels = 14.6666667;
    FcPatternGetDouble(defaults, FC_PIXEL_SIZE, 0, &fonts->pixels);
    FcPatternDestroy(defaults);
    if (!isfinite(fonts->pixels) || fonts->pixels < 4 || fonts->pixels > 256) {
        ztty_fonts_free(fonts); return NULL;
    }
    int primary = ztty_fonts_select(fonts, (const unsigned char *)"M", 1, 0);
    if (primary < 0) { ztty_fonts_free(fonts); return NULL; }
    FT_Face face = fonts->faces[primary].face;
    if (FT_Load_Char(face, 'M', FT_LOAD_DEFAULT)) { ztty_fonts_free(fonts); return NULL; }
    float scale = fonts->faces[primary].scale;
    metrics->cell_width = ceilf(face->glyph->advance.x / 64.0f * scale);
    metrics->cell_height = ceilf(face->size->metrics.height / 64.0f * scale);
    metrics->ascent = ceilf(face->size->metrics.ascender / 64.0f * scale);
    if (metrics->cell_width < 1 || metrics->cell_height < 1) {
        ztty_fonts_free(fonts); return NULL;
    }
    return fonts;
}

int ztty_fonts_shape(ZttyFonts *fonts, unsigned id, const unsigned char *utf8,
                     size_t length, ZttyShapedGlyph *output, unsigned capacity) {
    if (!fonts || id >= fonts->count || length > TEXT_LIMIT ||
        (length && !utf8) || !output) return -1;
    hb_buffer_t *buffer = hb_buffer_create();
    if (!buffer) return -1;
    hb_buffer_add_utf8(buffer, (const char *)utf8, (int)length, 0, (int)length);
    hb_buffer_set_cluster_level(buffer, HB_BUFFER_CLUSTER_LEVEL_MONOTONE_GRAPHEMES);
    hb_buffer_guess_segment_properties(buffer);
    hb_shape(fonts->faces[id].hb, buffer, NULL, 0);
    unsigned count = 0;
    hb_glyph_info_t *info = hb_buffer_get_glyph_infos(buffer, &count);
    hb_glyph_position_t *positions = hb_buffer_get_glyph_positions(buffer, NULL);
    if (!hb_buffer_allocation_successful(buffer) || count > capacity) {
        hb_buffer_destroy(buffer); return -2;
    }
    float scale = fonts->faces[id].scale / 64.0f;
    for (unsigned i = 0; i < count; ++i) {
        output[i] = (ZttyShapedGlyph){info[i].codepoint, info[i].cluster,
            positions[i].x_advance * scale, positions[i].y_advance * scale,
            positions[i].x_offset * scale, positions[i].y_offset * scale};
    }
    hb_buffer_destroy(buffer);
    return (int)count;
}

int ztty_fonts_glyph(ZttyFonts *fonts, unsigned id, uint32_t glyph,
                     ZttyGlyphBitmap *output) {
    if (!fonts || id >= fonts->count || !output) return 0;
    FontFace *font = &fonts->faces[id];
    if (FT_Load_Glyph(font->face, glyph, FT_LOAD_COLOR)) return 0;
    if ((font->style & 1) && !(font->face->style_flags & FT_STYLE_FLAG_BOLD))
        FT_GlyphSlot_Embolden(font->face->glyph);
    if ((font->style & 2) && !(font->face->style_flags & FT_STYLE_FLAG_ITALIC))
        FT_GlyphSlot_Oblique(font->face->glyph);
    if (font->face->glyph->format != FT_GLYPH_FORMAT_BITMAP &&
        FT_Render_Glyph(font->face->glyph, FT_RENDER_MODE_NORMAL)) return 0;
    FT_GlyphSlot slot = font->face->glyph;
    FT_Bitmap *bitmap = &slot->bitmap;
    if (bitmap->width > 1024 || bitmap->rows > 1024 ||
        (bitmap->pixel_mode != FT_PIXEL_MODE_GRAY &&
         bitmap->pixel_mode != FT_PIXEL_MODE_MONO &&
         bitmap->pixel_mode != FT_PIXEL_MODE_BGRA)) return 0;
    *output = (ZttyGlyphBitmap){bitmap->buffer, (int)bitmap->width,
        (int)bitmap->rows, bitmap->pitch, bitmap->pixel_mode,
        slot->bitmap_left, slot->bitmap_top, font->scale};
    return 1;
}

uint32_t ztty_unicode_script(uint32_t scalar) {
    return hb_unicode_script(hb_unicode_funcs_get_default(), scalar);
}
