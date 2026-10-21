//! A free TCP port from the OS (M2-T01, §4.1).
//!
//! `bind(127.0.0.1:0)` asks the kernel for a port nobody is using, and reading `local_addr`
//! returns which one it chose. The listener is then dropped, and the port is handed to the app
//! through its port variable.
//!
//! There is an unavoidable race here, and it is the plan's design rather than an oversight: the
//! port is free when we ask and could be taken by another process before the app binds it. The
//! alternative — holding the listener until the app starts — would make the app's bind fail
//! against us. A run that loses the race fails to boot and says so, which is a boot failure the
//! reader can retry, not a wrong verdict.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};

/// A port the kernel says is currently free, bound to the loopback address.
///
/// Loopback on purpose: the app under test is reached by substance on this host and must not be
/// reachable from the network (§4.1, ADR-0003).
///
/// # Errors
///
/// Returns the IO error when the kernel refuses to allocate a loopback listener.
pub fn free_port() -> Result<u16, io::Error> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
    let port = listener.local_addr()?.port();
    // Dropped here, which is what releases the port for the app to bind.
    drop(listener);
    Ok(port)
}
