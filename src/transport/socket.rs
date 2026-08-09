//! Socket helpers built on `socket2`.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

use crate::error::OmtError;
use crate::types::{NETWORK_RECEIVE_BUFFER, NETWORK_SEND_BUFFER};

/// Apply OMT-recommended TCP options (NODELAY, keepalive, buffers).
pub fn configure_stream(stream: &TcpStream) -> Result<(), OmtError> {
    stream.set_nodelay(true)?;
    let sock = socket2::SockRef::from(stream);
    sock.set_keepalive(true)?;
    let _ = sock.set_send_buffer_size(NETWORK_SEND_BUFFER);
    let _ = sock.set_recv_buffer_size(NETWORK_RECEIVE_BUFFER);
    Ok(())
}

/// Create a TCP socket suitable for OMT (dual-stack capable for IPv6).
pub fn create_tcp_socket(addr: SocketAddr) -> Result<Socket, OmtError> {
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    if addr.is_ipv6() {
        let _ = socket.set_only_v6(false);
    }
    socket.set_nodelay(true)?;
    socket.set_keepalive(true)?;
    let _ = socket.set_send_buffer_size(NETWORK_SEND_BUFFER);
    let _ = socket.set_recv_buffer_size(NETWORK_RECEIVE_BUFFER);
    Ok(socket)
}

/// Bind and listen on `addr`.
pub fn listen(addr: SocketAddr) -> Result<Socket, OmtError> {
    let socket = create_tcp_socket(addr)?;
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    Ok(socket)
}

/// Connect with optional timeout, applying OMT socket options.
pub fn connect(addr: SocketAddr, timeout: Option<Duration>) -> Result<TcpStream, OmtError> {
    let socket = create_tcp_socket(addr)?;
    match timeout {
        Some(t) => socket.connect_timeout(&addr.into(), t)?,
        None => socket.connect(&addr.into())?,
    }
    let stream: TcpStream = socket.into();
    configure_stream(&stream)?;
    Ok(stream)
}

/// Convert a listening `socket2::Socket` into a std `TcpListener`.
pub fn into_listener(socket: Socket) -> Result<TcpListener, OmtError> {
    Ok(socket.into())
}
