"""Record and verify the renderer used by an owned X11 test window."""


def record_renderer(connection, window, requested=None):
    def property_text(name):
        value = window.get_full_property(connection.intern_atom(name), 0)
        return bytes(value.value).decode("utf-8", errors="strict") if value is not None else None

    actual = property_text("_ZTTY_RENDERER")
    driver = property_text("_ZTTY_GPU_DRIVER")
    if requested in ("gpu", "software") and actual != requested:
        raise AssertionError(f"requested {requested}, actual renderer is {actual!r}; driver={driver!r}")
    return {"window_id": window.id, "requested": requested, "actual": actual, "driver": driver}
