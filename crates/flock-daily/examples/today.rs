//! `cargo run -p flock-daily --features fetch --example today`
fn main() {
    match flock_daily::http::fetch_today() {
        Ok(o) => {
            println!("day         {}", o.daily.day);
            println!("height      {}", o.daily.height);
            println!("hash        {}", o.daily.hash_hex);
            println!("seed        {:#018x} ({})", o.daily.seed, o.daily.seed);
            println!("cross-check {}", if o.cross_checked { "blockstream.info agrees" } else { "second source unreachable" });
            println!("requests    {}", o.requests);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
