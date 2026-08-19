fn main() {
    if reactor_desktop_lib::run_worker_from_args() {
        return;
    }
    reactor_desktop_lib::run();
}
