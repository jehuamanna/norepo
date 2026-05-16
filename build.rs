// `src/main.rs` uses `include_dir!("$CARGO_MANIFEST_DIR/assets/editor-bridge/dist")`
// to embed the editor-bridge JS into the desktop binary. Cargo doesn't track
// files read by proc-macros, so without these hints `cargo build` reuses a
// cached `main.o` even when the dist contents have changed — shipping a stale
// snapshot. Re-emit on every file under dist so a `just build-bridge` always
// propagates into the next bundle.
use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/editor-bridge/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    walk(&dist);

    // Seed SDLC skills are embedded via `include_dir!` in
    // `src/plugins/skill/seed.rs` so a shipped binary can install them
    // without the user pointing at the source folder. Same Cargo-can't-
    // see-into-proc-macros caveat as the editor-bridge tree above.
    let seeds = Path::new(env!("CARGO_MANIFEST_DIR")).join("seed-skills-updated");
    println!("cargo:rerun-if-changed={}", seeds.display());
    walk(&seeds);
}

fn walk(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            walk(&path);
        }
    }
}
