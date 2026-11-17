# Source from interactive Zsh: source /path/to/ztty/shell/ztty.zsh
[[ -o interactive && -t 1 ]] || return 0
(( ${+_ztty_zsh_loaded} )) && return 0
typeset -g _ztty_zsh_loaded=1

_ztty_zsh_cwd() {
    emulate -L zsh
    local LC_ALL=C cwd_value=$PWD encoded= ch host=${HOST-} i
    case $host in *[!a-zA-Z0-9.-]*) host= ;; esac
    for (( i=1; i<=${#cwd_value}; i++ )); do
        ch=$cwd_value[i]
        case $ch in
            [a-zA-Z0-9/._~-]) encoded+=$ch ;;
            *) builtin printf -v ch '%%%02X' "'$ch"; encoded+=$ch ;;
        esac
    done
    builtin printf '\033]7;file://%s%s\007' "$host" "$encoded"
}

_ztty_zsh_precmd() {
    local result=$?
    if [[ -t 1 ]]; then
        builtin printf '\033]133;D;%s\007' "$result"
        _ztty_zsh_cwd
    fi
    return 0
}

_ztty_zsh_prompt() {
    local result=$?
    local begin=$'%{\e]133;A\a%}' end=$'%{\e]133;B\a%}' secondary=$'%{\e]133;A;k=s\a%}'
    [[ ${PS1-} == "$begin"* ]] || PS1=$begin${PS1-}
    [[ ${PS1-} == *"$end" ]] || PS1+=${end}
    [[ ${PS2-} == "$secondary"* ]] || PS2=$secondary${PS2-}
    return 0
}

_ztty_zsh_preexec() {
    [[ -t 1 ]] && builtin printf '\033]133;C\007'
    return 0
}

typeset -ga precmd_functions preexec_functions
precmd_functions=(_ztty_zsh_precmd "${precmd_functions[@]}" _ztty_zsh_prompt)
preexec_functions=("${preexec_functions[@]}" _ztty_zsh_preexec)
