extern crate anyhow;
extern crate clap;
extern crate cpal;
extern crate ctrlc;

use clap::arg;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let app = clap::Command::new("audioping")
        .arg(arg!(-l --list "List audio devices"))
        .arg(arg!(-v --volume [VOLUME] "Signal amplitude multiplier 0-100, default: 50"))
        .arg(arg!(-s --sensitivity [SENSITIVITY] "Signal amplitude required to trigger, default: 1.0"))
        .arg(arg!(-i --input [IN] "The input audio device to use"))
        .arg(arg!(-o --output [OUT] "The output audio device to use"));

    let matches = app.get_matches();
    let input_device = matches.value_of("input");
    let output_device = matches.value_of("output");

    let volume_str = matches.value_of("volume").unwrap_or("50");
    let volume = volume_str.parse::<f32>()?.max(0f32).min(100f32) / 100f32;
    let sensitivity_str = matches.value_of("sensitivity").unwrap_or("1");
    let sensitivity = sensitivity_str.parse::<f32>()?.max(0f32).min(2f32);

    let (tx, rx) = channel();
    ctrlc::set_handler(move || tx.send(()).expect("Could not send signal on channel."))
        .expect("Error setting Ctrl-C handler");

    let host = cpal::default_host();

    if matches.is_present("list") {
        println!("Input devices:");
        for device in host.input_devices()? {
            println!("  {}", device.name()?);
        }
        println!("Output devices:");
        for device in host.output_devices()? {
            println!("  {}", device.name()?);
        }
        return Ok(());
    }

    let input = if input_device.is_none() {
        host.default_input_device()
    } else {
        host.input_devices()?.find(|x| {
            x.name()
                .map(|y| y == input_device.unwrap())
                .unwrap_or(false)
        })
    }
    .expect("failed to find input device");

    let output = if output_device.is_none() {
        host.default_output_device()
    } else {
        host.output_devices()?.find(|x| {
            x.name()
                .map(|y| y == output_device.unwrap())
                .unwrap_or(false)
        })
    }
    .expect("failed to find output device");

    println!("Using input device: \"{}\"", input.name()?);
    println!("Using output device: \"{}\"", output.name()?);

    let config: cpal::StreamConfig = output.default_output_config()?.into();
    let sample_rate = config.sample_rate.0 as f32;
    let channels = config.channels as usize;
    let signal_active = Arc::new(AtomicBool::new(false));
    let signal_active2 = Arc::clone(&signal_active);
    let signal_start = Arc::new(AtomicU64::new(0));
    let signal_start2 = Arc::clone(&signal_start);

    let start_time = Instant::now();

    // Input loop
    let input_data_fn = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        let frame_start = start_time.elapsed().as_nanos() as u64;

        let (mut delay_count, mut amplitude_count) = (0u64, 0u32);
        let (mut min, mut max) = (Option::<f32>::None, Option::<f32>::None);
        for frame in data.chunks(channels) {
            let sample = &frame[0];
            min = min.and_then(|x| Some(x.min(*sample))).or(Some(*sample));
            max = max.and_then(|x| Some(x.max(*sample))).or(Some(*sample));
            if max.unwrap() - min.unwrap() > sensitivity {
                amplitude_count += 1;
            }
            if amplitude_count <= 10 {
                delay_count += 1;
            }
        }
        let amplitude = max.unwrap_or(0f32) - min.unwrap_or(0f32);
        if amplitude_count > 10 {
            let was_active = signal_active.swap(false, Ordering::SeqCst);
            if was_active {
                let mut delay_ms = (frame_start - signal_start.load(Ordering::SeqCst)) as f32 / 1000.0;
                delay_ms += delay_count as f32 * 1000.0 / sample_rate;
                println!("Delay: {}ms, Signal: {}", delay_ms, amplitude);
            }
        } else if amplitude_count == 0 {
            let was_active = signal_active.swap(true, Ordering::SeqCst);
            if !was_active {
                signal_start.store(0, Ordering::SeqCst);
            }
        }
    };

    // Output loop
    let output_data_fn = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        if signal_active2.load(Ordering::SeqCst) {
            // Produce a sinusoid at the specified amplitude.
            let mut sample_clock = 0f32;
            let mut next_value = move || {
                sample_clock = (sample_clock + 1.0) % sample_rate;
                (sample_clock * 440.0 * 2.0 * PI / sample_rate).sin()
            };
            for frame in data.chunks_mut(channels) {
                let value = next_value() * volume;
                for sample in frame.iter_mut() {
                    *sample = value;
                }
            }
            let _ = signal_start2.compare_exchange(
                0,
                start_time.elapsed().as_nanos() as u64,
                Ordering::SeqCst,
                Ordering::Relaxed,
            );
        } else {
            // Mute
            for frame in data.chunks_mut(channels) {
                for sample in frame.iter_mut() {
                    *sample = 0f32;
                }
            }
        }
    };

    println!(
        "Attempting to build both streams with f32 samples and `{:?}`.",
        config
    );
    let input_stream = input.build_input_stream(&config, input_data_fn, err_fn)?;
    let output_stream = output.build_output_stream(&config, output_data_fn, err_fn)?;
    println!("Successfully built streams.");

    println!("Starting the input and output streams");
    input_stream.play()?;
    output_stream.play()?;

    println!("Measuring latency... Press Ctrl-C to stop");
    rx.recv().expect("Could not receive from channel.");
    drop(input_stream);
    drop(output_stream);
    println!("Done!");
    Ok(())
}

fn err_fn(err: cpal::StreamError) {
    eprintln!("an error occurred on stream: {}", err);
}
