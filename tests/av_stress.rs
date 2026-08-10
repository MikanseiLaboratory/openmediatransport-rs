//! Audio continues while video decode is busy.

use openmediatransport::{Codec, FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender};
use std::thread;
use std::time::{Duration, Instant};
use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

#[test]
fn audio_independent_of_video_decode() {
    let mut sender = Sender::create("AvSrc", FrameType::VIDEO | FrameType::AUDIO).expect("sender");
    let url = format!("omt://127.0.0.1:{}", sender.port());
    let session = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO | FrameType::AUDIO,
            auto_reconnect: false,
            connect_timeout: Duration::from_secs(2),
            ..ReceiverConfig::default()
        },
    )
    .expect("session");

    for _ in 0..200 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.video_subscribed() && sender.audio_subscribed() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    if !sender.video_subscribed() || !sender.audio_subscribed() {
        sender.force_subscribe(true, true, false);
    }

    let width = 320i32;
    let height = 180i32;
    let stride = (width as usize) * 2;
    let uyvy = vec![128u8; stride * height as usize];
    let mut enc = VmxCodec::new(VmxConfig {
        width,
        height,
        profile: Profile::OmtLq,
        color_space: Default::default(),
    })
    .unwrap();
    enc.encode_uyvy(&uyvy, stride).unwrap();
    let mut bitstream = vec![0u8; 1 << 20];
    let len = enc.save_to(&mut bitstream).unwrap();

    let samples = 480usize;
    let channels = 2usize;
    let mut pcm = vec![0u8; channels * samples * 4];
    for (i, chunk) in pcm.chunks_exact_mut(4).enumerate() {
        let s = ((i % 64) as f32) / 64.0;
        chunk.copy_from_slice(&s.to_le_bytes());
    }

    let t0 = Instant::now();
    let mut audio_got = 0u64;
    let mut video_got = 0u64;
    let mut i = 0i64;
    while t0.elapsed() < Duration::from_millis(800) {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        i += 1;
        let v = MediaFrame {
            frame_type: FrameType::VIDEO,
            timestamp: i * 10_000_000,
            codec: Codec::Vmx1 as i32,
            width,
            height,
            frame_rate_n: 60_000,
            frame_rate_d: 1_001,
            aspect_ratio: 16.0 / 9.0,
            data: bitstream[..len].to_vec(),
            ..Default::default()
        };
        let _ = sender.send_video(v);
        let a = MediaFrame {
            frame_type: FrameType::AUDIO,
            timestamp: i * 10_000_000,
            codec: Codec::Fpa1 as i32,
            sample_rate: 48_000,
            channels: channels as i32,
            samples_per_channel: samples as i32,
            data: pcm.clone(),
            ..Default::default()
        };
        let _ = sender.send_audio(a);

        while session.try_recv_audio().is_some() {
            audio_got += 1;
        }
        while session.try_recv_video().is_some() {
            video_got += 1;
        }
        thread::sleep(Duration::from_millis(2));
    }

    let stats = session.statistics();
    eprintln!("av stress: audio={audio_got} video={video_got} stats={stats:?}");
    assert!(audio_got >= 5, "audio starved: {audio_got}");
    assert!(video_got >= 1, "no video");
    session.disconnect();
}
