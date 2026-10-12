fn main() {
    // generate_context! embeds the icons but cargo doesn't track them; rebuild when they change
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
