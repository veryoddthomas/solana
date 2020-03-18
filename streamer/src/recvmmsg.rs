//! The `recvmmsg` module provides recvmmsg() API implementation

use crate::packet::Packet;
pub use solana_perf::packet::NUM_RCVMMSGS;
use std::cmp;
use std::io;
use std::net::UdpSocket;

#[cfg(not(target_os = "linux"))]
pub fn recv_mmsg(socket: &UdpSocket, packets: &mut [Packet]) -> io::Result<(usize, usize)> {
    let mut i = 0;
    let count = cmp::min(NUM_RCVMMSGS, packets.len());
    let mut total_size = 0;
    for p in packets.iter_mut().take(count) {
        p.meta.size = 0;
        match socket.recv_from(&mut p.data) {
            Err(_) if i > 0 => {
                break;
            }
            Err(e) => {
                return Err(e);
            }
            Ok((nrecv, from)) => {
                total_size += nrecv;
                p.meta.size = nrecv;
                p.meta.set_addr(&from);
                if i == 0 {
                    socket.set_nonblocking(true)?;
                }
            }
        }
        i += 1;
    }
    Ok((total_size, i))
}

#[cfg(target_os = "linux")]
pub fn recv_mmsg(sock: &UdpSocket, packets: &mut [Packet]) -> io::Result<(usize, usize)> {
    use libc::{
        c_void, iovec, mmsghdr, recvmmsg, sockaddr_in, socklen_t, time_t, timespec, MSG_WAITFORONE,
    };
    use nix::sys::socket::InetAddr;
    use std::mem;
    use std::os::unix::io::AsRawFd;

    let mut hdrs: [mmsghdr; NUM_RCVMMSGS] = unsafe { mem::zeroed() };
    let mut iovs: [iovec; NUM_RCVMMSGS] = unsafe { mem::zeroed() };
    let mut addr: [sockaddr_in; NUM_RCVMMSGS] = unsafe { mem::zeroed() };
    let addrlen = mem::size_of_val(&addr) as socklen_t;

    let sock_fd = sock.as_raw_fd();

    let count = cmp::min(iovs.len(), packets.len());

    for i in 0..count {
        iovs[i].iov_base = packets[i].data.as_mut_ptr() as *mut c_void;
        iovs[i].iov_len = packets[i].data.len();

        hdrs[i].msg_hdr.msg_name = &mut addr[i] as *mut _ as *mut _;
        hdrs[i].msg_hdr.msg_namelen = addrlen;
        hdrs[i].msg_hdr.msg_iov = &mut iovs[i];
        hdrs[i].msg_hdr.msg_iovlen = 1;
    }
    let mut ts = timespec {
        tv_sec: 1 as time_t,
        tv_nsec: 0,
    };

    let mut total_size = 0;
    let npkts =
        match unsafe { recvmmsg(sock_fd, &mut hdrs[0], count as u32, MSG_WAITFORONE, &mut ts) } {
            -1 => return Err(io::Error::last_os_error()),
            n => {
                for i in 0..n as usize {
                    let mut p = &mut packets[i];
                    p.meta.size = hdrs[i].msg_len as usize;
                    total_size += p.meta.size;
                    let inet_addr = InetAddr::V4(addr[i]);
                    p.meta.set_addr(&inet_addr.to_std());
                }
                n as usize
            }
        };

    Ok((total_size, npkts))
}

#[cfg(test)]
mod tests {
    use crate::packet::PACKET_DATA_SIZE;
    use crate::recvmmsg::*;
    use std::time::{Duration, Instant};

    const TEST_NUM_MSGS: usize = 32;
    #[test]
    pub fn test_recv_mmsg_one_iter() {
        let reader = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let addr = reader.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let saddr = sender.local_addr().unwrap();
        let sent = TEST_NUM_MSGS - 1;
        for _ in 0..sent {
            let data = [0; PACKET_DATA_SIZE];
            sender.send_to(&data[..], &addr).unwrap();
        }

        let mut packets = vec![Packet::default(); TEST_NUM_MSGS];
        let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
        assert_eq!(sent, recv);
        for i in 0..recv {
            assert_eq!(packets[i].meta.size, PACKET_DATA_SIZE);
            assert_eq!(packets[i].meta.addr(), saddr);
        }
    }

    #[test]
    pub fn test_recv_mmsg_multi_iter() {
        let reader = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let addr = reader.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let saddr = sender.local_addr().unwrap();
        let sent = TEST_NUM_MSGS + 10;
        for _ in 0..sent {
            let data = [0; PACKET_DATA_SIZE];
            sender.send_to(&data[..], &addr).unwrap();
        }

        let mut packets = vec![Packet::default(); TEST_NUM_MSGS];
        let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
        assert_eq!(TEST_NUM_MSGS, recv);
        for i in 0..recv {
            assert_eq!(packets[i].meta.size, PACKET_DATA_SIZE);
            assert_eq!(packets[i].meta.addr(), saddr);
        }

        let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
        assert_eq!(sent - TEST_NUM_MSGS, recv);
        for i in 0..recv {
            assert_eq!(packets[i].meta.size, PACKET_DATA_SIZE);
            assert_eq!(packets[i].meta.addr(), saddr);
        }
    }

    #[test]
    pub fn test_recv_mmsg_multi_iter_timeout() {
        let reader = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let addr = reader.local_addr().unwrap();
        reader.set_read_timeout(Some(Duration::new(5, 0))).unwrap();
        reader.set_nonblocking(false).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let saddr = sender.local_addr().unwrap();
        let sent = TEST_NUM_MSGS;
        for _ in 0..sent {
            let data = [0; PACKET_DATA_SIZE];
            sender.send_to(&data[..], &addr).unwrap();
        }

        let start = Instant::now();
        let mut packets = vec![Packet::default(); TEST_NUM_MSGS];
        let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
        assert_eq!(TEST_NUM_MSGS, recv);
        for i in 0..recv {
            assert_eq!(packets[i].meta.size, PACKET_DATA_SIZE);
            assert_eq!(packets[i].meta.addr(), saddr);
        }
        reader.set_nonblocking(true).unwrap();

        let _recv = recv_mmsg(&reader, &mut packets[..]);
        assert!(start.elapsed().as_secs() < 5);
    }

    #[test]
    pub fn test_recv_mmsg_multi_addrs() {
        let reader = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let addr = reader.local_addr().unwrap();

        let sender1 = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let saddr1 = sender1.local_addr().unwrap();
        let sent1 = TEST_NUM_MSGS - 1;

        let sender2 = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let saddr2 = sender2.local_addr().unwrap();
        let sent2 = TEST_NUM_MSGS + 1;

        for _ in 0..sent1 {
            let data = [0; PACKET_DATA_SIZE];
            sender1.send_to(&data[..], &addr).unwrap();
        }

        for _ in 0..sent2 {
            let data = [0; PACKET_DATA_SIZE];
            sender2.send_to(&data[..], &addr).unwrap();
        }

        let mut packets = vec![Packet::default(); TEST_NUM_MSGS];

        let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
        assert_eq!(TEST_NUM_MSGS, recv);
        for i in 0..sent1 {
            assert_eq!(packets[i].meta.size, PACKET_DATA_SIZE);
            assert_eq!(packets[i].meta.addr(), saddr1);
        }

        for i in sent1..recv {
            assert_eq!(packets[i].meta.size, PACKET_DATA_SIZE);
            assert_eq!(packets[i].meta.addr(), saddr2);
        }

        let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
        assert_eq!(sent1 + sent2 - TEST_NUM_MSGS, recv);
        for i in 0..recv {
            assert_eq!(packets[i].meta.size, PACKET_DATA_SIZE);
            assert_eq!(packets[i].meta.addr(), saddr2);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    pub fn test_recv_mmsg_batch_size() {
        let reader = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let addr = reader.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind");

        const TEST_BATCH_SIZE: usize = 64;
        let sent = TEST_BATCH_SIZE;

        let mut elapsed_in_max_batch = 0;
        (0..1000).for_each(|_| {
            for _ in 0..sent {
                let data = [0; PACKET_DATA_SIZE];
                sender.send_to(&data[..], &addr).unwrap();
            }
            let mut packets = vec![Packet::default(); TEST_BATCH_SIZE];
            let now = Instant::now();
            let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
            elapsed_in_max_batch += now.elapsed().as_nanos();
            assert_eq!(TEST_BATCH_SIZE, recv);
        });

        let mut elapsed_in_small_batch = 0;
        (0..1000).for_each(|_| {
            for _ in 0..sent {
                let data = [0; PACKET_DATA_SIZE];
                sender.send_to(&data[..], &addr).unwrap();
            }
            let mut packets = vec![Packet::default(); 4];
            let mut recv = 0;
            let now = Instant::now();
            while let Ok(num) = recv_mmsg(&reader, &mut packets[..]) {
                recv += num.1;
                if recv >= TEST_BATCH_SIZE {
                    break;
                }
            }
            elapsed_in_small_batch += now.elapsed().as_nanos();
            assert_eq!(TEST_BATCH_SIZE, recv);
        });

        assert!(elapsed_in_max_batch <= elapsed_in_small_batch);
    }
}