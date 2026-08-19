//! Socket helpers built on `socket2`.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

use crate::error::OmtError;
use crate::types::{
    NETWORK_RECEIVE_BUFFER, NETWORK_SEND_BUFFER, NETWORK_SEND_RECEIVE_BUFFER, NETWORK_SEND_TIMEOUT,
};

/// Apply OMT-recommended TCP options for a **receiver** connection.
pub fn configure_stream(stream: &TcpStream) -> Result<(), OmtError> {
    configure_stream_buffers(stream, NETWORK_SEND_BUFFER, NETWORK_RECEIVE_BUFFER)
}

/// Apply OMT-recommended TCP options for a **sender-side** peer connection.
///
/// Matches libomtnet: 64 KiB send + 64 KiB receive on the sending channel.
/// A write timeout prevents a dead peer from stalling every other client.
pub fn configure_sender_peer_stream(stream: &TcpStream) -> Result<(), OmtError> {
    configure_stream_buffers(stream, NETWORK_SEND_BUFFER, NETWORK_SEND_RECEIVE_BUFFER)?;
    let _ = stream.set_write_timeout(Some(NETWORK_SEND_TIMEOUT));
    let sock = socket2::SockRef::from(stream);
    let _ = sock.set_linger(Some(Duration::ZERO));
    Ok(())
}

/// Apply TCP options with explicit buffer sizes.
pub fn configure_stream_buffers(
    stream: &TcpStream,
    send_buffer: usize,
    recv_buffer: usize,
) -> Result<(), OmtError> {
    stream.set_nodelay(true)?;
    let sock = socket2::SockRef::from(stream);
    sock.set_keepalive(true)?;
    let _ = sock.set_send_buffer_size(send_buffer);
    let _ = sock.set_recv_buffer_size(recv_buffer);
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
    socket.set_tcp_nodelay(true)?;
    socket.set_keepalive(true)?;
    let _ = socket.set_send_buffer_size(NETWORK_SEND_BUFFER);
    let _ = socket.set_recv_buffer_size(NETWORK_RECEIVE_BUFFER);
    Ok(socket)
}

/// Bind and listen on `addr`.
///
/// Does **not** enable `SO_REUSEADDR`. On Windows that option allows multiple
/// sockets to bind the same port and steal accepts — which breaks parallel
/// tests (and production senders) sharing 6400..=6600.
pub fn listen(addr: SocketAddr) -> Result<Socket, OmtError> {
    let socket = create_tcp_socket(addr)?;
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
    // `connect_timeout` toggles non-blocking internally; restore blocking I/O.
    let _ = stream.set_nonblocking(false);
    configure_stream(&stream)?;
    // Media sockets only: RST on close so a sender is not left half-open.
    let sock = socket2::SockRef::from(&stream);
    let _ = sock.set_linger(Some(Duration::ZERO));
    Ok(stream)
}

/// Convert a listening `socket2::Socket` into a std `TcpListener`.
pub fn into_listener(socket: Socket) -> Result<TcpListener, OmtError> {
    Ok(socket.into())
}
