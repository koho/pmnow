mod pms;

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose};
use ring::rand::SecureRandom;
use ring::signature::{self, KeyPair};
use rumqttc::{AsyncClient, MqttOptions, QoS};
use std::io::Write;
use std::sync::{Arc, atomic};
use std::time::SystemTime;
use std::{env, fs, io};
use tokio::sync::mpsc;
use tokio::time::{self, Duration, Instant};

static PM25_H: atomic::AtomicU64 = atomic::AtomicU64::new(0);
static PM100_H: atomic::AtomicU64 = atomic::AtomicU64::new(0);
static RESET_H: atomic::AtomicBool = atomic::AtomicBool::new(true);

#[tokio::main]
async fn main() -> Result<()> {
    let mut sensor = pms::PMS::new(env::args().nth(1).expect("Please provide a serial port"))?;
    let name = env::args().nth(2).unwrap_or_default();
    let name = name.as_bytes();
    if name.len() > 255 {
        anyhow::bail!("Sensor name too long");
    }

    let rng = ring::rand::SystemRandom::new();
    let key_pair = Arc::new(
        if let Ok(key) = fs::read("pmnow.key") {
            signature::Ed25519KeyPair::from_pkcs8(key.as_slice())
        } else {
            let pkcs8_bytes = signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
            fs::write("pmnow.key", pkcs8_bytes.as_ref())?;
            signature::Ed25519KeyPair::from_pkcs8(pkcs8_bytes.as_ref())
        }
        .unwrap(),
    );

    let id = general_purpose::URL_SAFE.encode(key_pair.public_key().as_ref());
    println!("ID: {}", id);
    let topic = format!("pmnow/{id}");

    let mut client_id = [0u8; 12];
    rng.fill(&mut client_id).unwrap();
    let opts = MqttOptions::new(hex::encode(client_id), "broker.emqx.io", 1883);
    let (client, mut eventloop) = AsyncClient::new(opts, 10);
    let (tx, mut rx) = mpsc::channel(1);
    tokio::spawn(async move {
        loop {
            let _ = eventloop.poll().await;
        }
    });
    let key_pair_cloned = key_pair.clone();
    tokio::spawn(async move {
        let sleep = time::sleep(Duration::from_secs(
            3600 - SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                % 3600,
        ));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                Some(msg) = rx.recv() => {
                    let _ = client.publish(&topic, QoS::AtMostOnce, true, msg).await;
                }
                _ = &mut sleep => {
                    let pm25 = format!("{:.2}", f64::from_bits(PM25_H.load(atomic::Ordering::Relaxed)));
                    let pm100 = format!("{:.2}", f64::from_bits(PM100_H.load(atomic::Ordering::Relaxed)));
                    RESET_H.store(true, atomic::Ordering::Relaxed);
                    let id = id.clone();
                    let key_pair = key_pair_cloned.clone();
                    tokio::spawn(async move {
                        let client = reqwest::Client::new();
                        let ts = SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH).unwrap()
                            .as_secs().to_string();
                        let payload = format!("{ts}-{pm25}-{pm100}");
                        let sig = key_pair.sign(payload.as_bytes());
                        let sig = general_purpose::STANDARD.encode(sig.as_ref());
                        for _ in 0..5 {
                            if client.post("https://pmnow.netlify.app/api/hourly")
                                .form(&[("id", &id), ("ts", &ts), ("pm25", &pm25), ("pm10", &pm100), ("sig", &sig)])
                                .send()
                                .await.is_ok() {
                                return;
                            }
                            time::sleep(Duration::from_secs(5)).await;
                        }
                    });
                    sleep.as_mut().reset(Instant::now() + Duration::from_secs(3600));
                }
            }
        }
    });

    let mut buf = vec![0; 1 + name.len() + 4 + 8 + 105];
    let buf = buf.as_mut_slice();
    buf[0] = name.len() as u8;
    if name.len() > 0 {
        buf[1..name.len() + 1].copy_from_slice(name);
    }
    let mut pm25_count = 0u64;
    let mut pm100_count = 0u64;
    let offset = name.len() + 1;
    loop {
        let frame = sensor.get_frame().await?;
        print!("\x1B[2K\r");
        print!("PM: {:?} {:?}", frame.pm25_env, frame.pm100_env);
        let _ = io::stdout().flush();

        if RESET_H
            .compare_exchange(
                true,
                false,
                atomic::Ordering::Acquire,
                atomic::Ordering::Relaxed,
            )
            .is_ok()
        {
            PM25_H.store(frame.pm25_env as u64, atomic::Ordering::Relaxed);
            PM100_H.store(frame.pm100_env as u64, atomic::Ordering::Relaxed);
            pm25_count = 1;
            pm100_count = 1;
        } else {
            if PM25_H
                .fetch_update(atomic::Ordering::SeqCst, atomic::Ordering::SeqCst, |v| {
                    let count = pm25_count as f64;
                    let avg = (count / (count + 1.0)) * f64::from_bits(v)
                        + frame.pm25_env as f64 / (count + 1.0);
                    Some(avg.to_bits())
                })
                .is_ok()
            {
                pm25_count += 1;
            }
            if PM100_H
                .fetch_update(atomic::Ordering::SeqCst, atomic::Ordering::SeqCst, |v| {
                    let count = pm100_count as f64;
                    let avg = (count / (count + 1.0)) * f64::from_bits(v)
                        + frame.pm100_env as f64 / (count + 1.0);
                    Some(avg.to_bits())
                })
                .is_ok()
            {
                pm100_count += 1;
            }
        }

        buf[offset..offset + 2].copy_from_slice(&frame.pm25_env.to_be_bytes());
        buf[offset + 2..offset + 4].copy_from_slice(&frame.pm100_env.to_be_bytes());
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_millis() as u64;
        buf[offset + 4..offset + 12].copy_from_slice(&ts.to_be_bytes());
        let sig = key_pair.sign(&buf[..offset + 12]);
        let buf_len = offset + 12 + sig.as_ref().len();
        buf[offset + 12..buf_len].copy_from_slice(sig.as_ref());
        tx.try_send(buf[..buf_len].to_vec())?;
    }
}
