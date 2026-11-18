# Source from an interactive Bash 4.4+ session: source /path/to/ztty/shell/ztty.bash
# PS0 marks execution without replacing DEBUG traps or readline bindings.
[[ $- == *i* && -t 1 ]] || return 0
[[ ${_ztty_bash_loaded-} != "$BASHPID" ]] || return 0
if (( BASH_VERSINFO[0] < 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] < 4) )); then
    printf '%s\n' 'ztty shell integration requires Bash 4.4 or newer (PS0).' >&2
    return 1
fi
_ztty_bash_loaded=$BASHPID

_ztty_bash_cwd() {
    local LC_ALL=C cwd_value=$PWD encoded= ch host=${HOSTNAME-} i
    case $host in *[!a-zA-Z0-9.-]*) host= ;; esac
    for ((i=0; i<${#cwd_value}; i++)); do
        ch=${cwd_value:i:1}
        case $ch in
            [a-zA-Z0-9/._~-]) encoded+=$ch ;;
            *) printf -v encoded '%s%%%02X' "$encoded" "'$ch" ;;
        esac
    done
    printf '\033]7;file://%s%s\007' "$host" "$encoded"
}

_ztty_bash_precmd() {
    local result=$?
    if [[ -t 1 ]]; then
        printf '\033]133;D;%s\007' "$result"
        _ztty_bash_cwd
    fi
    return "$result"
}

_ztty_bash_prompt() {
    local result=$?
    local begin=$'\[\e]133;A\a\]' end=$'\[\e]133;B\a\]' secondary=$'\[\e]133;A;k=s\a\]'
    [[ ${PS1-} == "$begin"* ]] || PS1=$begin${PS1-}
    [[ ${PS1-} == *"$end" ]] || PS1+=${end}
    [[ ${PS2-} == "$secondary"* ]] || PS2=$secondary${PS2-}
    return "$result"
}

# Keep each existing PROMPT_COMMAND array entry, or the complete original string,
# between the status capture and prompt wrapping. Themes can still replace PS1.
# Bash before 5.1 executes PROMPT_COMMAND as one string, not an array.
if (( BASH_VERSINFO[0] < 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] < 1) )); then
    PROMPT_COMMAND=$'_ztty_bash_precmd\n'"${PROMPT_COMMAND-}"$'\n_ztty_bash_prompt'
elif [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a '* ]]; then
    PROMPT_COMMAND=(_ztty_bash_precmd "${PROMPT_COMMAND[@]}" _ztty_bash_prompt)
else
    PROMPT_COMMAND=(_ztty_bash_precmd "${PROMPT_COMMAND-}" _ztty_bash_prompt)
fi
# PS0 may be exported into a nested interactive shell. Preserve its suffix
# without prepending the execution marker twice.
[[ ${PS0-} == $'\e]133;C\a'* ]] || PS0=$'\e]133;C\a'${PS0-}
