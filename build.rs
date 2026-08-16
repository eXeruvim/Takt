fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=ui");
    slint_build::compile("ui/app_window.slint")?;
    Ok(())
}
