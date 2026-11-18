/* Independent Win32 resize control: no terminal, PTY, or ztty runtime. */
#define WIN32_LEAN_AND_MEAN
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0A00
#endif
#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static const wchar_t CLASS_NAME[] = L"Win32ResizeControl";
static HDC memory_dc;
static HBITMAP bitmap;
static HGDIOBJ original_bitmap;
static int surface_width, surface_height, failed;

static int resize_surface(int width, int height) {
    if (width == surface_width && height == surface_height) return 1;
    if (width <= 0 || height <= 0 ||
        (uint64_t)(unsigned)width * (unsigned)height > 16u * 1024u * 1024u) return 0;
    BITMAPINFO info = {0};
    info.bmiHeader.biSize = sizeof(info.bmiHeader);
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    void *bits = NULL;
    HBITMAP next = CreateDIBSection(memory_dc, &info, DIB_RGB_COLORS, &bits, NULL, 0);
    if (!next || !bits) {
        if (next) DeleteObject(next);
        return 0;
    }
    /* This newly allocated DIB has never been submitted to GDI. */
    uint32_t *pixels = bits;
    for (int y = 0; y < height; ++y) {
        for (int x = 0; x < width; ++x) {
            uint32_t color = 0x00101820u;
            if (x >= 171 && x <= 179 && y >= 20 && y <= 37) color = 0x00ff0000u;
            if (x >= 171 && x <= 179 && y >= 38 && y <= 55) color = 0x000000ffu;
            pixels[(size_t)y * (size_t)width + (size_t)x] = color;
        }
    }
    HGDIOBJ previous = SelectObject(memory_dc, next);
    if (!previous || previous == HGDI_ERROR) {
        DeleteObject(next);
        return 0;
    }
    if (!original_bitmap) original_bitmap = previous;
    if (bitmap) DeleteObject(bitmap);
    bitmap = next;
    surface_width = width;
    surface_height = height;
    return 1;
}

static LRESULT CALLBACK window_proc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    switch (message) {
    case WM_SIZE:
        if (wparam != SIZE_MINIMIZED) {
            if (!resize_surface((int)LOWORD(lparam), (int)HIWORD(lparam))) {
                failed = 1;
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            } else {
                InvalidateRect(hwnd, NULL, FALSE);
            }
        }
        return 0;
    case WM_ERASEBKGND:
        return 1;
    case WM_PAINT: {
        PAINTSTRUCT paint;
        HDC target = BeginPaint(hwnd, &paint);
        if (!target || !bitmap || !BitBlt(target, 0, 0, surface_width, surface_height,
                                         memory_dc, 0, 0, SRCCOPY)) failed = 1;
        EndPaint(hwnd, &paint);
        if (failed) PostMessageW(hwnd, WM_CLOSE, 0, 0);
        return 0;
    }
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    default:
        /* Keep default frame geometry and default valid-rectangle/copy policy. */
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
}

int wmain(void) {
    typedef HRESULT (WINAPI *SetDwmAttribute)(HWND, DWORD, LPCVOID, DWORD);
    HINSTANCE instance = GetModuleHandleW(NULL);
    HMODULE dwm = NULL;
    HWND hwnd = NULL;
    int registered = 0;
    HRESULT dark_result = E_FAIL;
    if (!SetProcessDpiAwarenessContext((DPI_AWARENESS_CONTEXT)(intptr_t)-4)) goto failure;
    memory_dc = CreateCompatibleDC(NULL);
    if (!memory_dc) goto failure;
    WNDCLASSEXW cls = {0};
    cls.cbSize = sizeof(cls);
    cls.style = CS_HREDRAW | CS_VREDRAW;
    cls.lpfnWndProc = window_proc;
    cls.hInstance = instance;
    cls.hCursor = LoadCursorW(NULL, IDC_IBEAM);
    cls.lpszClassName = CLASS_NAME;
    if (!RegisterClassExW(&cls)) goto failure;
    registered = 1;
    hwnd = CreateWindowExW(0, CLASS_NAME, L"Win32 resize control", WS_OVERLAPPEDWINDOW,
                           CW_USEDEFAULT, CW_USEDEFAULT, 196, 229, NULL, NULL, instance, NULL);
    if (!hwnd) goto failure;
    dwm = LoadLibraryW(L"dwmapi.dll");
    if (!dwm) goto failure;
    FARPROC address = GetProcAddress(dwm, "DwmSetWindowAttribute");
    SetDwmAttribute set_attribute = NULL;
    _Static_assert(sizeof(set_attribute) == sizeof(address), "Win32 function pointer size");
    memcpy(&set_attribute, &address, sizeof(set_attribute));
    if (!set_attribute) goto failure;
    const BOOL dark = TRUE;
    dark_result = set_attribute(hwnd, 20, &dark, sizeof(dark));
    if (FAILED(dark_result)) goto failure;
    printf("{\"pid\":%lu,\"dpi_awareness\":-4,\"class_style\":3,"
           "\"window_style\":%lu,\"background_brush_null\":true,"
           "\"creation_position\":{\"x\":\"CW_USEDEFAULT\",\"y\":\"CW_USEDEFAULT\"},"
           "\"dark_attribute\":20,\"dark_value\":1,\"dark_hresult\":%ld,"
           "\"valid_rect_policy\":\"default\",\"nocopybits_added\":false}\n",
           (unsigned long)GetCurrentProcessId(), (unsigned long)WS_OVERLAPPEDWINDOW,
           (long)dark_result);
    fflush(stdout);
    ShowWindow(hwnd, SW_SHOW);
    MSG message;
    int result;
    while ((result = (int)GetMessageW(&message, NULL, 0, 0)) > 0) {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
    if (result < 0) failed = 1;
    goto cleanup;
failure:
    fprintf(stderr, "Win32 resize control setup failed: Win32=%lu, DWM=%ld\n",
            (unsigned long)GetLastError(), (long)dark_result);
    failed = 1;
cleanup:
    if (hwnd && IsWindow(hwnd)) DestroyWindow(hwnd);
    if (memory_dc && original_bitmap) SelectObject(memory_dc, original_bitmap);
    if (bitmap) DeleteObject(bitmap);
    if (memory_dc) DeleteDC(memory_dc);
    if (dwm) FreeLibrary(dwm);
    if (registered) UnregisterClassW(CLASS_NAME, instance);
    return failed ? 1 : 0;
}
