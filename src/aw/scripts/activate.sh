# Source this file in Bash or cosh-shell's Bash host to enable the AW Qoder entry.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    printf '%s\n' 'Use: source /path/to/src/aw/scripts/activate.sh --allow-unrecoverable' >&2
    exit 1
fi
if [[ "${1:-}" != --allow-unrecoverable || "$#" != 1 ]]; then
    printf '%s\n' 'Activation requires --allow-unrecoverable for native output replacement.' >&2
    return 1
fi
if declare -F qoder >/dev/null || alias qoder >/dev/null 2>&1; then
    printf '%s\n' 'An existing qoder function or alias is already defined; activation stopped.' >&2
    return 1
fi
_AW_SESSION_ENTRY="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/session.py"
qoder() {
    python3 "$_AW_SESSION_ENTRY" --allow-unrecoverable -- "$@"
}
printf '%s\n' 'AW enabled for this shell. Run qoder; Ctrl+B then p opens Provider details.'
printf '%s\n' 'Undo: unset -f qoder; unset _AW_SESSION_ENTRY'
