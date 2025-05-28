function fish_prompt
    set -l exe ::TURBOFISH::
    set -l last_status $status

    set -q TMPDIR
    and set -l tmpdir $TMPDIR
    or set -l tmpdir /tmp

    if not set -q __turbo_prompt_state_file
        mkdir -p $tmpdir/turbo.fish
        set -g __turbo_prompt_state_file (mktemp --tmpdir turbo.fish/XXXXXXXXXX)
        RUST_BACKTRACE=1 $exe serve $__turbo_prompt_state_file &
        set -g __turbo_fish_pid $last_pid
        disown
    end
    # Jobs display
    if set -q AUTOJUMP_SOURCED
        # Autojump special case: check if there are jobs besides the `autojump`
        # job, since that one is (briefly) backgrounded every time we `cd`
        set bg_jobs (jobs -c | string match -v --regex '(Command|autojump)' | wc -l)
        [ "$bg_jobs" -eq 0 ]
        and set bg_jobs # clear it out so it doesn't show when `0`
    else
        set bg_jobs (jobs -p | wc -l)
        [ "$bg_jobs" -eq 0 ]
        and set bg_jobs # clear it out so it doesn't show when `0`
    end
    __TURBO_FISH_PID=$__turbo_fish_pid __TURBO_FISH_LAST_STATUS=$last_status __TURBO_FISH_BG_JOBS=$bg_jobs \
        $exe render $__turbo_prompt_state_file
end

function __turbo_fish_refresh --on-event fish_postexec
    if set -q __turbo_fish_pid
        if not kill -s USR1 $__turbo_fish_pid 2>/dev/null
            rm $__turbo_prompt_state_file
            set -e __turbo_fish_pid __turbo_prompt_state_file
        end
    end
end

function __turbo_fish_signal_handler --on-signal SIGUSR1
    commandline -f repaint >/dev/null 2>/dev/null
end
