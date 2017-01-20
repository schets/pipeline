extern crate crossbeam;
extern crate pipeline;
extern crate time;

use pipeline::queue::multiqueue::{MultiReader, MultiWriter, multiqueue};

use time::precise_time_ns;

use crossbeam::scope;

use std::sync::atomic::{AtomicUsize, Ordering, fence};
use std::sync::Barrier;

#[inline(never)]
fn waste_50_ns(val: &AtomicUsize) {
    val.store(0, Ordering::Release);
    fence(Ordering::SeqCst);
}

fn recv(bar: &Barrier, reader: MultiReader<Option<u64>>) {
    bar.wait();
    let mut v = Vec::with_capacity(100000);
    loop {
        if let Some(popped) = reader.pop() {
            match popped {
                None => break,
                Some(pushed) => {
                    let current_time = precise_time_ns();
                    if (current_time >= pushed) {
                        v.push(current_time - pushed);
                    }
                }
            }
        }
    }
    println!("DONE!!!!");
    for val in v {
         println!("{}", val);
    }
}

fn Send(bar: &Barrier, writer: MultiWriter<Option<u64>>, num_push: usize, num_us: usize) {
    bar.wait();
    let val: AtomicUsize = AtomicUsize::new(0);
    println!("WRITING!!!!");
    for _ in 0..num_push {
        loop {
            let topush = Some(precise_time_ns());
            if let Ok(_) =  writer.push(topush) {
                break;
            }
        }
        for _ in 0..(num_us*20) {
            waste_50_ns(&val);
        }
    }
    println!("WRITING-NONE");
    writer.push(None);
    println!("WRITTEN!!!!");
}

fn main() {
    let (writer, reader) = multiqueue(20000);
    let bar = Barrier::new(2);
    let bref = &bar;
    scope(|scope| {
        scope.spawn(move || {
            Send(bref, writer, 100000, 40);
        });
        recv(bref, reader);
    });
}