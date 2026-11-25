//! Send a DMA-BUF descriptor back over the full-duplex control socket.
//! The corresponding bounded metadata is ordered on the helper's output pipe.
use rustix::net::{
    self, RecvAncillaryBuffer, RecvAncillaryMessage, SendAncillaryBuffer, SendAncillaryMessage,
};
use std::{
    io::{self, IoSlice, IoSliceMut},
    mem::MaybeUninit,
    os::fd::{AsFd, OwnedFd},
};

pub fn send_buffer(socket: impl AsFd, fd: impl AsFd) -> io::Result<()> {
    let mut bytes = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let fds = [fd.as_fd()];
    let mut control = SendAncillaryBuffer::new(&mut bytes);
    control.push(SendAncillaryMessage::ScmRights(&fds));
    let sent = net::sendmsg(
        socket,
        &[IoSlice::new(&[1])],
        &mut control,
        net::SendFlags::NOSIGNAL,
    )?;
    if sent != 1 {
        return Err(io::Error::other("Failed to send GPU buffer"));
    }
    Ok(())
}
pub fn receive_buffer(socket: impl AsFd) -> io::Result<OwnedFd> {
    let mut bytes = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = RecvAncillaryBuffer::new(&mut bytes);
    let mut marker = [0];
    let msg = net::recvmsg(
        socket,
        &mut [IoSliceMut::new(&mut marker)],
        &mut control,
        net::RecvFlags::CMSG_CLOEXEC | net::RecvFlags::DONTWAIT,
    )?;
    if msg.bytes == 1 && marker == [1] && !msg.flags.contains(net::ReturnFlags::CTRUNC) {
        for cmsg in control.drain() {
            if let RecvAncillaryMessage::ScmRights(mut fds) = cmsg
                && let Some(fd) = fds.next()
                && fds.next().is_none()
            {
                return Ok(fd);
            }
        }
    }
    Err(io::Error::other("Missing GPU buffer descriptor"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
    };
    #[test]
    fn descriptor_survives_sender_close() {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        let (resource, mut peer) = UnixStream::pair().unwrap();
        send_buffer(&sender, resource).unwrap();
        let mut imported = UnixStream::from(receive_buffer(&receiver).unwrap());
        peer.write_all(b"surface").unwrap();
        let mut bytes = [0; 7];
        imported.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"surface");
        sender.write_all(&[1]).unwrap();
        assert!(receive_buffer(&receiver).is_err());
    }
}
