//! Loopback send/receive integration test.

use openmediatransport::{
    FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender, Settings,
    protocol::metadata::SUBSCRIBE_VIDEO,
};
use std::thread;
use std::time::Duration;

#[test]
fn metadata_subscribe_and_video_roundtrip() {
    let mut sender =
        Sender::create("TestSrc", FrameType::VIDEO | FrameType::METADATA).expect("sender");
    let port = sender.port();
    let (start, end) = Settings::global()
        .lock()
        .expect("settings lock")
        .network_port_range();
    assert!(
        (start..=end).contains(&port),
        "sender port {port} outside configured range {start}..={end}"
    );

    let url = format!("omt://127.0.0.1:{port}");
    let session = ReceiverSession::connect(
        &url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO | FrameType::METADATA,
            ..ReceiverConfig::default()
        },
    )
    .expect("rx");

    for _ in 0..100 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.video_subscribed() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    if !sender.video_subscribed() {
        sender.force_subscribe(true, false, true);
    }
    assert!(sender.video_subscribed());

    let width = 16i32;
    let height = 16i32;
    let stride = (width as usize) * 2;
    let payload = vec![128u8; stride * height as usize];
    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 12345,
        codec: openmediatransport::Codec::Uyvy as i32,
        width,
        height,
        stride: stride as i32,
        frame_rate_n: 60,
        frame_rate_d: 1,
        aspect_ratio: 1.0,
        data: payload,
        ..Default::default()
    };
    sender.send_video(frame).expect("send");

    let mut got = None;
    for _ in 0..200 {
        if let Some(f) = session.try_recv_video()
            && f.timestamp == 12345
        {
            got = Some(f);
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frame = got.expect("decoded uncompressed UYVY");
    assert_eq!(frame.width, width as u32);
    assert_eq!(frame.height, height as u32);
    assert_eq!(frame.pixels.len(), (width * height * 4) as usize);
    let _ = SUBSCRIBE_VIDEO;
    session.disconnect();
}

#[test]
fn vmx_colorbar_loopback_decodes_bgra() {
    use openmediatransport::Codec;
    use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

    let mut sender = Sender::create("VmxSrc", FrameType::VIDEO).expect("sender");
    let port = sender.port();
    let url = format!("omt://127.0.0.1:{port}");
    let session = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            connect_timeout: Duration::from_secs(2),
            ..ReceiverConfig::default()
        },
    )
    .expect("session");

    for _ in 0..100 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.video_subscribed() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !sender.video_subscribed() {
        sender.force_subscribe(true, false, false);
    }

    let width = 128i32;
    let height = 128i32;
    let stride = (width as usize) * 2;
    let mut uyvy = vec![128u8; stride * height as usize];
    for y in 0..height as usize {
        for x in (0..width as usize).step_by(2) {
            let o = y * stride + x * 2;
            uyvy[o] = 128;
            uyvy[o + 1] = 16 + ((x + y) % 220) as u8;
            uyvy[o + 2] = 128;
            uyvy[o + 3] = 16 + ((x + 1 + y) % 220) as u8;
        }
    }
    let mut enc = VmxCodec::new(VmxConfig {
        width,
        height,
        profile: Profile::OmtLq,
        color_space: Default::default(),
    })
    .unwrap();
    enc.encode_uyvy(&uyvy, stride).unwrap();
    let mut bitstream = vec![0u8; 2 << 20];
    let len = enc.save_to(&mut bitstream).unwrap();

    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 10_000_000,
        codec: Codec::Vmx1 as i32,
        width,
        height,
        frame_rate_n: 60_000,
        frame_rate_d: 1_001,
        aspect_ratio: 16.0 / 9.0,
        data: bitstream[..len].to_vec(),
        ..Default::default()
    };
    sender.send_video(frame).unwrap();

    let mut got = None;
    for _ in 0..200 {
        if let Some(f) = session.try_recv_video() {
            // Parallel tests can briefly race; wait for our own timestamp.
            if f.timestamp == 10_000_000 {
                got = Some(f);
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frame = got.expect("decoded BGRA frame");
    assert_eq!(frame.timestamp, 10_000_000);
    assert_eq!(frame.width, width as u32);
    assert_eq!(frame.height, height as u32);
    assert_eq!(frame.pixels.len(), (width * height * 4) as usize);
    session.disconnect();
}

#[test]
fn vmx_preview_loopback_decodes_eighth_bgra() {
    use openmediatransport::Codec;
    use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

    let mut sender = Sender::create("VmxPreviewSrc", FrameType::VIDEO).expect("sender");
    let port = sender.port();
    let url = format!("omt://127.0.0.1:{port}");
    let session = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            preview: true,
            connect_timeout: Duration::from_secs(2),
            ..ReceiverConfig::default()
        },
    )
    .expect("session");

    for _ in 0..100 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.video_subscribed() && sender.preview() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !sender.video_subscribed() {
        sender.force_subscribe(true, false, false);
    }
    if !sender.preview() {
        sender.force_preview(true);
    }
    assert!(sender.preview());

    let width = 128i32;
    let height = 128i32;
    let stride = (width as usize) * 2;
    let mut uyvy = vec![128u8; stride * height as usize];
    for y in 0..height as usize {
        for x in (0..width as usize).step_by(2) {
            let o = y * stride + x * 2;
            uyvy[o] = 128;
            uyvy[o + 1] = 16 + ((x + y) % 220) as u8;
            uyvy[o + 2] = 128;
            uyvy[o + 3] = 16 + ((x + 1 + y) % 220) as u8;
        }
    }
    let mut enc = VmxCodec::new(VmxConfig {
        width,
        height,
        profile: Profile::OmtLq,
        color_space: Default::default(),
    })
    .unwrap();
    enc.encode_uyvy(&uyvy, stride).unwrap();
    let mut bitstream = vec![0u8; 2 << 20];
    let full_len = enc.save_to(&mut bitstream).unwrap();
    let preview_len = enc.get_encoded_preview_length();
    assert!(preview_len < full_len);

    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 20_000_000,
        codec: Codec::Vmx1 as i32,
        width,
        height,
        frame_rate_n: 60_000,
        frame_rate_d: 1_001,
        aspect_ratio: 16.0 / 9.0,
        data: bitstream[..full_len].to_vec(),
        ..Default::default()
    };
    sender.send_video(frame).unwrap();

    let mut got = None;
    for _ in 0..200 {
        if let Some(f) = session.try_recv_video() {
            // Parallel tests can briefly race; wait for our own timestamp.
            if f.timestamp == 20_000_000 {
                got = Some(f);
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frame = got.expect("decoded preview BGRA frame");
    assert_eq!(frame.timestamp, 20_000_000);
    assert_eq!(frame.width, 16);
    assert_eq!(frame.height, 16);
    assert_eq!(frame.pixels.len(), 16 * 16 * 4);
    session.disconnect();
}

#[cfg(feature = "wgpu")]
mod gpu {
    use openmediatransport::{
        FrameType, GpuVideoContext, MediaFrame, ReceiverConfig, ReceiverSession, Sender,
        VideoTextureMeta,
    };
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use vmx::gpu;

    fn headless_ctx() -> Option<(GpuVideoContext, wgpu::Device, wgpu::Queue)> {
        let (_, _, device, queue) = gpu::request_headless_device()?;
        let ctx = GpuVideoContext {
            device: Arc::new(device.clone()),
            queue: Arc::new(queue.clone()),
        };
        Some((ctx, device, queue))
    }

    fn wait_video_sub(sender: &mut Sender) {
        for _ in 0..100 {
            let _ = sender.poll_accept();
            let _ = sender.poll_peer_metadata();
            if sender.video_subscribed() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        sender.force_subscribe(true, false, false);
    }

    fn sample_bgra(width: i32, height: i32) -> Vec<u8> {
        let stride = width as usize * 4;
        let mut bgra = vec![0u8; stride * height as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let o = y * stride + x * 4;
                bgra[o] = (x.wrapping_mul(13) % 220 + 16) as u8;
                bgra[o + 1] = (y.wrapping_mul(7) % 200 + 20) as u8;
                bgra[o + 2] = ((x + y) % 180 + 30) as u8;
                bgra[o + 3] = 255;
            }
        }
        bgra
    }

    fn psnr_bgra(a: &[u8], b: &[u8]) -> f64 {
        assert_eq!(a.len(), b.len());
        let mut sse = 0.0f64;
        let n = (a.len() / 4) * 3;
        if n == 0 {
            return 100.0;
        }
        for i in (0..a.len()).step_by(4) {
            for c in 0..3 {
                let d = f64::from(a[i + c]) - f64::from(b[i + c]);
                sse += d * d;
            }
        }
        if sse == 0.0 {
            return 100.0;
        }
        10.0 * (255.0 * 255.0 * n as f64 / sse).log10()
    }

    fn assert_psnr(a: &[u8], b: &[u8], min_db: f64, label: &str) {
        let psnr = psnr_bgra(a, b);
        assert!(
            psnr >= min_db,
            "{label}: PSNR {psnr:.2} dB is below {min_db:.1} dB"
        );
    }

    #[test]
    fn gpu_receive_matches_cpu_loopback() {
        let Some((ctx, device, queue)) = headless_ctx() else {
            eprintln!("skip: no wgpu adapter");
            return;
        };

        let mut sender = Sender::create("GpuRxSrc", FrameType::VIDEO).expect("sender");
        let port = sender.port();
        let url = format!("omt://127.0.0.1:{port}");
        let cpu = ReceiverSession::connect(
            &url,
            ReceiverConfig {
                frame_types: FrameType::VIDEO,
                connect_timeout: Duration::from_secs(2),
                ..ReceiverConfig::default()
            },
        )
        .expect("cpu rx");
        let gpu_rx = ReceiverSession::connect(
            &url,
            ReceiverConfig {
                frame_types: FrameType::VIDEO,
                connect_timeout: Duration::from_secs(2),
                gpu: Some(ctx),
                ..ReceiverConfig::default()
            },
        )
        .expect("gpu rx");

        wait_video_sub(&mut sender);

        let width = 64i32;
        let height = 64i32;
        let stride = (width as usize) * 4;
        let bgra = sample_bgra(width, height);
        let frame = MediaFrame {
            frame_type: FrameType::VIDEO,
            timestamp: 30_000_000,
            codec: openmediatransport::Codec::Bgra as i32,
            width,
            height,
            stride: stride as i32,
            frame_rate_n: 60,
            frame_rate_d: 1,
            data: bgra,
            ..Default::default()
        };
        sender.send_video(frame).expect("send");

        let mut cpu_got = None;
        let mut gpu_got = None;
        for _ in 0..300 {
            if cpu_got.is_none()
                && let Some(f) = cpu.try_recv_video()
                && f.timestamp == 30_000_000
            {
                cpu_got = Some(f);
            }
            if gpu_got.is_none()
                && let Some(f) = gpu_rx.try_recv_video_gpu()
                && f.timestamp == 30_000_000
            {
                gpu_got = Some(f);
            }
            if cpu_got.is_some() && gpu_got.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let cpu_frame = cpu_got.expect("cpu decoded");
        let gpu_frame = gpu_got.expect("gpu decoded");
        assert!(
            gpu_rx.try_recv_video().is_none(),
            "CPU recv must be None in GPU mode"
        );
        let gpu_pixels = gpu::read_texture_bgra(
            &device,
            &queue,
            &gpu_frame.texture,
            gpu_frame.width,
            gpu_frame.height,
        )
        .expect("readback");
        assert_psnr(
            cpu_frame.pixels.as_ref(),
            gpu_pixels.as_slice(),
            40.0,
            "GPU recv vs CPU",
        );
        cpu.disconnect();
        gpu_rx.disconnect();
    }

    #[test]
    fn gpu_preview_receive() {
        let Some((ctx, device, queue)) = headless_ctx() else {
            eprintln!("skip: no wgpu adapter");
            return;
        };

        let mut sender = Sender::create("GpuPreviewSrc", FrameType::VIDEO).expect("sender");
        let port = sender.port();
        let url = format!("omt://127.0.0.1:{port}");
        let session = ReceiverSession::connect(
            url,
            ReceiverConfig {
                frame_types: FrameType::VIDEO,
                preview: true,
                connect_timeout: Duration::from_secs(2),
                gpu: Some(ctx),
                ..ReceiverConfig::default()
            },
        )
        .expect("session");

        for _ in 0..100 {
            let _ = sender.poll_accept();
            let _ = sender.poll_peer_metadata();
            if sender.video_subscribed() && sender.preview() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !sender.video_subscribed() {
            sender.force_subscribe(true, false, false);
        }
        if !sender.preview() {
            sender.force_preview(true);
        }

        let width = 128i32;
        let height = 128i32;
        let frame = MediaFrame {
            frame_type: FrameType::VIDEO,
            timestamp: 40_000_000,
            codec: openmediatransport::Codec::Bgra as i32,
            width,
            height,
            stride: (width as usize * 4) as i32,
            frame_rate_n: 60,
            frame_rate_d: 1,
            data: sample_bgra(width, height),
            ..Default::default()
        };
        sender.send_video(frame).expect("send");

        let mut got = None;
        for _ in 0..300 {
            if let Some(f) = session.try_recv_video_gpu()
                && f.timestamp == 40_000_000
            {
                got = Some(f);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let frame = got.expect("gpu preview");
        assert_eq!(frame.width, 16);
        assert_eq!(frame.height, 16);
        let pixels =
            gpu::read_texture_bgra(&device, &queue, &frame.texture, frame.width, frame.height)
                .expect("readback");
        assert_eq!(pixels.len(), 16 * 16 * 4);
        session.disconnect();
    }

    #[test]
    fn gpu_send_texture_matches_cpu_bgra() {
        let Some((ctx, device, queue)) = headless_ctx() else {
            eprintln!("skip: no wgpu adapter");
            return;
        };

        let mut sender = Sender::create("GpuTxSrc", FrameType::VIDEO).expect("sender");
        let port = sender.port();
        let url = format!("omt://127.0.0.1:{port}");
        let session = ReceiverSession::connect(
            &url,
            ReceiverConfig {
                frame_types: FrameType::VIDEO,
                connect_timeout: Duration::from_secs(2),
                ..ReceiverConfig::default()
            },
        )
        .expect("rx");
        wait_video_sub(&mut sender);

        let width = 32i32;
        let height = 32i32;
        let stride = (width as usize) * 4;
        let bgra = sample_bgra(width, height);
        let tex = gpu::upload_bgra_texture(&device, &queue, width as u32, height as u32, &bgra);

        sender
            .send_video_texture(
                &ctx,
                &tex,
                VideoTextureMeta {
                    width: width as u32,
                    height: height as u32,
                    timestamp: 50_000_000,
                    frame_rate_n: 60,
                    frame_rate_d: 1,
                    ..Default::default()
                },
            )
            .expect("send texture");

        let mut gpu_sent = None;
        for _ in 0..300 {
            if let Some(f) = session.try_recv_video()
                && f.timestamp == 50_000_000
            {
                gpu_sent = Some(f);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let gpu_sent = gpu_sent.expect("recv after GPU encode");

        let mut sender2 = Sender::create("GpuTxSrcCpu", FrameType::VIDEO).expect("sender2");
        let url2 = format!("omt://127.0.0.1:{}", sender2.port());
        let cpu_rx = ReceiverSession::connect(
            url2,
            ReceiverConfig {
                frame_types: FrameType::VIDEO,
                connect_timeout: Duration::from_secs(2),
                ..ReceiverConfig::default()
            },
        )
        .expect("cpu rx");
        wait_video_sub(&mut sender2);
        sender2
            .send_video(MediaFrame {
                frame_type: FrameType::VIDEO,
                timestamp: 50_000_000,
                codec: openmediatransport::Codec::Bgra as i32,
                width,
                height,
                stride: stride as i32,
                frame_rate_n: 60,
                frame_rate_d: 1,
                data: bgra,
                ..Default::default()
            })
            .expect("cpu send");
        let mut cpu_sent = None;
        for _ in 0..300 {
            if let Some(f) = cpu_rx.try_recv_video()
                && f.timestamp == 50_000_000
            {
                cpu_sent = Some(f);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let cpu_sent = cpu_sent.expect("cpu recv");
        assert_psnr(
            gpu_sent.pixels.as_ref(),
            cpu_sent.pixels.as_ref(),
            35.0,
            "GPU send vs CPU send",
        );
        session.disconnect();
        cpu_rx.disconnect();
    }
}
