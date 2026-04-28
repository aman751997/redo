// Global panic hook. On panic from any thread, restores the terminal
// (alt-screen exit, raw-mode disable) and kills the child process before
// re-raising, so the user's shell is left in a sane state.
//
// TODO: wire up terminal restore + child kill once the PTY layer exists.

pub fn install() {
    // No-op for now.
}
