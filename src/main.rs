extern crate anyhow;
extern crate clap;
extern crate cpal;
extern crate ctrlc;

use clap::arg;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let app = clap::Command::new("audioping")
        .arg(arg!(-l --list "List audio devices"))
        .arg(arg!(-i --input [IN] "The input audio device to use"))
        .arg(arg!(-o --output [OUT] "The output audio device to use"));

    let matches = app.get_matches();
    let input_device = matches.value_of("input");
    let output_device = matches.value_of("output");
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
    let beep_frames = Arc::new(AtomicU64::new(0));
    let beep_frames2 = Arc::clone(&beep_frames);
    let sample_rate = config.sample_rate.0 as f32;
    let channels = config.channels as usize;

    // Input loop
    let input_data_fn = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        let (mut total_count, mut amplitude_count) = (0u64, 0u32);
        let (mut min, mut max) = (Option::<f32>::None, Option::<f32>::None);
        for frame in data.chunks(channels) {
            let sample = &frame[0];
            min = min.and_then(|x| Some(x.min(*sample))).or(Some(*sample));
            max = max.and_then(|x| Some(x.max(*sample))).or(Some(*sample));
            if max.unwrap() - min.unwrap() > 1f32 {
                if amplitude_count > 10 {
                    break;
                }
                amplitude_count += 1;
            }
            total_count += 1;
        }
        if amplitude_count > 10 {
            let delay_frames = beep_frames.swap(0, Ordering::SeqCst);
            if delay_frames > 0 {
                let delay_ms =
                    (delay_frames * data.chunks(channels).len() as u64 + total_count) as f32 * 1000.0 / sample_rate;
                println!("Delay: {} frames, {}ms", delay_frames, delay_ms);
            }
        } else if amplitude_count == 0 {
            beep_frames.store(1, Ordering::SeqCst);
        }
    };

    // Output loop
    let output_data_fn = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        if beep_frames2.load(Ordering::SeqCst) == 0 {
            for frame in data.chunks_mut(channels) {
                for sample in frame.iter_mut() {
                    *sample = 0f32;
                }
            }
        } else {
            // Produce a sinusoid of maximum amplitude.
            let mut sample_clock = 0f32;
            let mut next_value = move || {
                sample_clock = (sample_clock + 1.0) % sample_rate;
                (sample_clock * 440.0 * 2.0 * std::f32::consts::PI / sample_rate).sin()
            };
            for frame in data.chunks_mut(channels) {
                let value = cpal::Sample::from::<f32>(&next_value());
                for sample in frame.iter_mut() {
                    *sample = value;
                }
            }
            beep_frames2.fetch_add(1, Ordering::SeqCst);
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
