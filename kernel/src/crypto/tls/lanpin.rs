//! Devices on the local network: pinned trust instead of "ignore errors".
//!
//! A router at `https://192.168.178.1` cannot have a publicly trusted
//! certificate: no CA issues one for a private address, and the name on its
//! self-signed certificate is not the IP the user types. The result is
//! always `hostname mismatch` + `untrusted root`, a gap in the Web PKI.
//!
//! Skipping verification for private addresses is not an option: the LAN is
//! not a safe place, and a compromised device on it could impersonate the
//! router and collect its password. Instead this is trust on first use,
//! then pinned (the SSH model):
//!
//! 1. The user names the address explicitly (`set net.lan_devices`); the
//!    default is empty.
//! 2. Only a literal private address counts, never a name. A name could
//!    resolve elsewhere next time; an address in the URL cannot change.
//! 3. On first connect the leaf certificate's fingerprint is recorded. From
//!    then on it must be identical; a different one is a hard error even
//!    for an allowed address.
//!
//! The trade-off: the first connection is unauthenticated, everything after
//! it is not. An attacker must already be in place at first contact.
//!
//! Only the two errors an honest device necessarily triggers are forgiven.
//! Expired, unparsable or badly signed certificates stay fatal.
//!
//! Pins live in RAM and are lost on reboot. Persisting trust to disk is a
//! separate decision.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use super::certstore::CertError;

/// What was recorded: address -> fingerprint of the leaf certificate.
static PINS: Mutex<Vec<(String, [u8; 32])>> = Mutex::new(Vec::new());

/// Is `host` a literal private address that the user has allowed?
///
/// Both conditions are required: the address is listed in
/// `net.lan_devices`, and it really is private. The second check keeps a
/// typo in the configuration from silently allowing a public address.
fn is_allowed_device(host: &str) -> bool {
    let bare = host.split(':').next().unwrap_or(host);
    // Literal addresses only: a name may resolve differently next time.
    let Some(ip) = crate::intent::parse_ip_pub(bare) else { return false };
    if crate::intent::reach::classify_ip(ip) == crate::intent::reach::Reach::Public {
        return false;
    }
    let list = crate::config::get("net.lan_devices").unwrap_or_default();
    list.split(',').map(str::trim).any(|e| !e.is_empty() && e == bare)
}

/// May this error be forgiven for an allowed device?
///
/// Only the two a self-signed device certificate necessarily triggers.
/// Anything else means something is wrong that an honest device would not
/// show.
fn is_forgivable(e: CertError) -> bool {
    matches!(e, CertError::HostnameMismatch | CertError::UntrustedRoot)
}

/// The second chance. `Ok(())` means: let it through.
///
/// Called only after `verify_chain` has rejected the chain, so this cannot
/// allow anything the regular check would have allowed, nor forbid anything
/// it already forbade.
pub fn second_chance(host: &str, leaf_der: &[u8], why: CertError) -> Result<(), CertError> {
    if !is_forgivable(why) || !is_allowed_device(host) {
        return Err(why);
    }
    let bare = host.split(':').next().unwrap_or(host);
    let fp = super::sha256::sha256(leaf_der);

    let mut pins = PINS.lock();
    if let Some((_, known)) = pins.iter().find(|(h, _)| h == bare) {
        if *known == fp {
            return Ok(());
        }
        // The case pinning exists for: the address is allowed, but someone
        // else answers. Reject.
        crate::kprintln!(
            "[npk] LAN-ANHEFTUNG: {} zeigt ein ANDERES Zertifikat als beim ersten Mal.",
            bare);
        crate::kprintln!("[npk]   Das kann ein Geraetetausch sein — oder jemand dazwischen.");
        crate::kprintln!("[npk]   `set net.lan_devices` neu setzen loescht die Anheftung nicht;");
        crate::kprintln!("[npk]   dafuer braucht es einen Neustart. So ist es gemeint.");
        return Err(why);
    }

    // First contact. Trust is granted here without backing, so log it.
    crate::kprintln!("[npk] LAN-Geraet {} beim ERSTEN Mal angenommen ({:?}).", bare, why);
    crate::kprintln!("[npk]   Fingerabdruck {:02x}{:02x}{:02x}{:02x}… ab jetzt angeheftet.",
        fp[0], fp[1], fp[2], fp[3]);
    pins.push((String::from(bare), fp));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the two errors an honest device triggers are forgivable; all
    /// others stay fatal, even for an allowed address.
    #[test]
    fn only_the_two_unavoidable_errors_are_forgivable() {
        assert!(is_forgivable(CertError::HostnameMismatch));
        assert!(is_forgivable(CertError::UntrustedRoot));
        for e in [CertError::Expired, CertError::NotYetValid, CertError::ParseError,
                  CertError::SignatureInvalid, CertError::EmptyChain, CertError::NotCA,
                  CertError::KeyUsageInvalid, CertError::EkuInvalid,
                  CertError::PathLenExceeded, CertError::UnknownCriticalExt,
                  CertError::BadValidityDate] {
            assert!(!is_forgivable(e), "{e:?} darf nicht nachgelassen werden");
        }
    }
}
