# Generated from rabi's help-data.json; workspace names are queried on completion.
_rabi_candidates() {
    local rabi_context='' rabi_pending='' rabi_word rabi_count=0 rabi_stop=0 rabi_help=0 rabi_names
    local -a rabi_repo=()
    for rabi_word in "$@"; do
        if [ -n "$rabi_pending" ]; then
            [ "$rabi_pending" != repo ] || rabi_repo=(--repo "$rabi_word")
            rabi_pending=''
            continue
        fi
        if [ "$rabi_stop" -eq 0 ]; then
            case "$rabi_word" in
                --) [ "$rabi_context" != exec ] || return 0; rabi_stop=1; continue ;;
                --repo=*) rabi_repo=(--repo "${rabi_word#*=}"); continue ;;
                -R?*) rabi_repo=(--repo "${rabi_word#-R}"); continue ;;
            esac
            case "$rabi_context:$rabi_word" in
@VALUE_OPTIONS@
            esac
            case "$rabi_word" in -*) continue ;; esac
            case "$rabi_context:$rabi_word" in
@COMMANDS@
            esac
        fi
        rabi_count=$((rabi_count + 1))
    done
    if [ -n "$rabi_pending" ]; then
        return 0
    fi
    if [ "$rabi_context" = exec ] && [ "$rabi_count" -gt 0 ]; then
        return 0
    fi
    case "$rabi_context" in
@CANDIDATES@
    esac
    return 0
}
