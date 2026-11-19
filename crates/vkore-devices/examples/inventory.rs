fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&vkore_devices::discover()).expect("serialize inventory")
    );
}
