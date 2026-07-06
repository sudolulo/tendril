# Launch the Tendril console on the primary virtual terminal (appliance UX).
#
# Only on /dev/tty1, and guarded so the menu's "Open Linux shell" doesn't re-enter it. When the menu
# exits, the login session ends and getty logs back in — reopening the menu. Admins can use another
# VT (tty2–tty6) or SSH for a plain shell.
if [ -z "$TENDRIL_CONSOLE" ] && [ "$(tty)" = "/dev/tty1" ]; then
    export TENDRIL_CONSOLE=1
    exec tendril
fi
