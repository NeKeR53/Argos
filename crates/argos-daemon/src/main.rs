fn main() {
    println!(
        "argos-daemon {} (protocole v{})",
        env!("CARGO_PKG_VERSION"),
        argos_core::PROTOCOL_VERSION
    );
}
