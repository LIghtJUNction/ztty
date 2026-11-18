# Source from config.nu or an interactive session:
# source /path/to/ztty/shell/ztty.nu
# Use Nushell's native semantic zones and URI escaping; keep every user hook.
if $nu.is-interactive {
    $env.config.shell_integration.osc133 = true
    $env.config.shell_integration.osc7 = true
}
