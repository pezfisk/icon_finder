use std::env;
use std::process;

use icon_finder::find_icon;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() != 3 {
        eprintln!("Usage: {} <app-name> <resolution>", args[0]);
        eprintln!("Example: {} firefox 128", args[0]);
        process::exit(1);
    }

    let app = &args[1];
    let size: u32 = match args[2].parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("Error: resolution must be a positive integer (e.g. 128)");
            process::exit(1);
        }
    };

    let time = std::time::Instant::now();
    match find_icon(app, size) {
        Some(path) => println!("{}", path.display()),
        None => {
            eprintln!("Icon not found for '{}' at size {}px", app, size);
            process::exit(1);
        }
    }
    let elapsed = time.elapsed();
    println!("Time took to find icon: {:.2?}", elapsed);
}
