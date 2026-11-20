fn main() -> Result<(), Box<dyn std::error::Error>> {
    ztty_registry_check_zterm::exercise();
    println!("registry consumer passed");
    Ok(())
}
