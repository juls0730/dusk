fn main() {
    println!("cargo:rerun-if-changed=src/arch/x86_64/linker.ld");
}
