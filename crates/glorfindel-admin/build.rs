fn main() {
    println!("cargo:rerun-if-changed=src/admin.html");
    println!("cargo:rerun-if-changed=src/campaign.html");
    println!("cargo:rerun-if-changed=src/agentifier.html");
    println!("cargo:rerun-if-changed=src/jellyfin.html");
}
