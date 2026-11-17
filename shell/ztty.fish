# Source from interactive Fish 4+: source /path/to/ztty/shell/ztty.fish
# Fish emits OSC 133 itself. Keep its prompt and preexec/postexec hooks intact.
status is-interactive; or return 0
set -l major (string split . -- $version)[1]
if test "$major" -lt 4
    printf '%s\n' 'ztty shell integration requires Fish 4 or newer (native OSC 133).' >&2
    return 1
end
functions -q __ztty_fish_cwd; and return 0

# OSC 7 is not enabled by Fish for every TERM value. Report the directory
# independently so ztty can inherit it without claiming another terminal name.
function __ztty_fish_cwd --on-event fish_prompt
    set -l cwd_uri (string escape --style=url -- $PWD | string replace -a '%2F' '/')
    set -l host $hostname
    string match -qr '^[a-zA-Z0-9.-]*$' -- "$host"; or set host ''
    printf '\033]7;file://%s%s\007' "$host" "$cwd_uri"
end
