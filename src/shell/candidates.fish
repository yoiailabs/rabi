# Generated from rabi's help-data.json; workspace names are queried on completion.
function _rabi_candidates
    set -l rabi_context ''
    set -l rabi_pending ''
    set -l rabi_count 0
    set -l rabi_stop 0
    set -l rabi_help 0
    set -l rabi_repo
    for rabi_word in $argv
        if test -n "$rabi_pending"
            if test "$rabi_pending" = repo
                set rabi_repo --repo "$rabi_word"
            end
            set rabi_pending ''
            continue
        end
        if test $rabi_stop -eq 0
            switch "$rabi_word"
                case --
                    test "$rabi_context" != exec; or return 0
                    set rabi_stop 1
                    continue
                case '--repo=*'
                    set rabi_repo --repo (string sub -s 8 -- "$rabi_word")
                    continue
                case '-R*'
                    if test "$rabi_word" != -R
                        set rabi_repo --repo (string sub -s 3 -- "$rabi_word")
                        continue
                    end
            end
            switch "$rabi_context:$rabi_word"
@VALUE_OPTIONS@
            end
            if string match -q -- '-*' "$rabi_word"
                continue
            end
            switch "$rabi_context:$rabi_word"
@COMMANDS@
            end
        end
        set rabi_count (math $rabi_count + 1)
    end
    if test -n "$rabi_pending"
        return 0
    end
    if test "$rabi_context" = exec; and test $rabi_count -gt 0
        return 0
    end
    switch "$rabi_context"
@CANDIDATES@
    end
    return 0
end

function __rabi_complete
    set -l rabi_tokens (commandline -opc)
    set -e rabi_tokens[1]
    _rabi_candidates $rabi_tokens
end
complete -c rabi -f -a '(__rabi_complete)'
