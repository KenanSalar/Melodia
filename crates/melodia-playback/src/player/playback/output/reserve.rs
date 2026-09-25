//! Asking the sound server for a card before opening it, over `org.freedesktop.ReserveDevice1`.
//!
//! The session manager holds every card it drives under `…ReserveDevice1.Audio<N>` at a negative
//! priority, so an application at priority 0 outranks it: it asks the holder to let go, the holder
//! closes the card and hands the name over, and the card opens. Without it the open meets a card
//! the server still has and fails `EBUSY`.
//!
//! **The protocol is two-way.** Holding the name means answering `RequestRelease` too: an
//! application above us asks, and we close the card before saying yes, or it meets the same
//! `EBUSY` we just avoided. What closing means is the caller's, handed in as `release`.
//!
//! Blocking D-Bus, on `zbus::blocking` only; see the root manifest for why never its `tokio`.

use zbus::blocking::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::names::WellKnownName;

use melodia_core::error::describe;

use super::claim::ClaimError;

const INTERFACE: &str = "org.freedesktop.ReserveDevice1";

/// Ours. Anything asking at a higher one gets the card; the session manager asks below zero.
const PRIORITY: i32 = 0;

const APPLICATION_NAME: &str = "Melodia";

/// A held reservation. Dropping it gives the name back.
pub(super) struct Reservation {
    bus: Connection,
    name: WellKnownName<'static>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // After a release we granted, the name is already someone else's, so a refusal here is
        // the expected answer rather than a fault.
        if let Err(e) = self.bus.release_name(self.name.as_ref()) {
            log::debug!("audio: card reservation not released: {}", describe(&e));
        }
    }
}

/// Take card `card`'s reservation, asking whoever holds it to let go.
///
/// `Ok(None)` where there is no session bus: nothing is running that could hold the card, so
/// there is nobody to coordinate with and the open goes ahead. Only a holder that says no is a
/// refusal.
///
/// # Errors
///
/// [`ClaimError::Reserved`] when the holder outranks us, [`ClaimError::Io`] when the bus fails
/// part way through.
pub(super) fn acquire(
    card: i32,
    device_name: String,
    release: impl Fn() -> bool + Send + Sync + 'static,
) -> Result<Option<Reservation>, ClaimError> {
    let bus = match Connection::session() {
        Ok(bus) => bus,
        Err(e) => {
            log::debug!("audio: no session bus, opening the card unreserved: {}", describe(&e));
            return Ok(None);
        }
    };
    let name = WellKnownName::try_from(format!("{INTERFACE}.Audio{card}"))
        .map_err(|e| ClaimError::io("Invalid card reservation name", e))?;
    let path = format!("/org/freedesktop/ReserveDevice1/Audio{card}");

    // Served before the name is taken, so a request arriving the moment it is ours finds it.
    bus.object_server()
        .at(path.as_str(), Reservable { device_name, release: Box::new(release) })
        .map_err(|e| ClaimError::io("Failed to serve the card reservation", e))?;

    let polite = RequestNameFlags::DoNotQueue | RequestNameFlags::AllowReplacement;
    if owns(bus.request_name_with_flags(name.as_ref(), polite))? {
        return Ok(Some(Reservation { bus, name }));
    }

    let holder = uncached_proxy(&bus, name.clone(), path.as_str(), INTERFACE)
        .map_err(|e| ClaimError::io("Failed to reach the card's holder", e))?;
    let released: bool = holder
        .call("RequestRelease", &(PRIORITY,))
        .map_err(|e| ClaimError::io("The card's holder did not answer", e))?;
    if !released {
        let by = holder.get_property::<String>("ApplicationName").unwrap_or_default();
        return Err(ClaimError::Reserved { by });
    }

    let taking = polite | RequestNameFlags::ReplaceExisting;
    if owns(bus.request_name_with_flags(name.as_ref(), taking))? {
        Ok(Some(Reservation { bus, name }))
    } else {
        Err(ClaimError::Reserved { by: String::new() })
    }
}

/// A proxy that reads each property when asked. zbus's default fetches them all through
/// `Properties.GetAll` on the first read, which rtkit doesn't implement, so every read failed.
pub(super) fn uncached_proxy<'a>(
    bus: &Connection,
    destination: impl TryInto<zbus::names::BusName<'a>, Error = impl Into<zbus::Error>>,
    path: &'a str,
    interface: &'a str,
) -> zbus::Result<zbus::blocking::Proxy<'a>> {
    zbus::blocking::proxy::Builder::new(bus)
        .destination(destination)?
        .path(path)?
        .interface(interface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
}

fn owns(reply: zbus::Result<RequestNameReply>) -> Result<bool, ClaimError> {
    match reply {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(true),
        // zbus answers the bus's `Exists` reply with an error rather than handing it back, so a
        // held name arrives on the error arm.
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => {
            Ok(false)
        }
        Err(e) => Err(ClaimError::io("Failed to request the card reservation", e)),
    }
}

/// The object a holder serves at `/org/freedesktop/ReserveDevice1/Audio<N>`.
struct Reservable {
    device_name: String,
    release: Box<dyn Fn() -> bool + Send + Sync>,
}

#[zbus::interface(name = "org.freedesktop.ReserveDevice1")]
impl Reservable {
    /// Say yes to anything that outranks us, once the card is closed.
    fn request_release(&self, priority: i32) -> bool {
        priority > PRIORITY && (self.release)()
    }

    #[zbus(property)]
    #[expect(clippy::unused_self, reason = "a zbus property is a method whatever it reads")]
    fn priority(&self) -> i32 {
        PRIORITY
    }

    #[zbus(property)]
    #[expect(clippy::unused_self, reason = "a zbus property is a method whatever it reads")]
    fn application_name(&self) -> &str {
        APPLICATION_NAME
    }

    #[zbus(property)]
    fn application_device_name(&self) -> &str {
        &self.device_name
    }
}
