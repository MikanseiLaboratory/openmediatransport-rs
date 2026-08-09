//! Discovery example — browse LAN for `_omt._tcp` sources (libomtnet-compatible).

use std::time::Duration;

use openmediatransport::Discovery;

fn main() -> Result<(), openmediatransport::OmtError> {
    let mut discovery = Discovery::new()?;
    // Allow time for QM PTR query + multicast answers (Bonjour / Windows DNS-SD).
    discovery.refresh_for(Duration::from_secs(3))?;
    println!("Discovered {} source(s)", discovery.sources().len());
    for src in discovery.sources() {
        println!(
            "  - {}  port={}  addrs={:?}  url={}",
            src.instance_name(),
            src.port,
            src.addresses,
            src.to_url()
        );
    }
    Ok(())
}
