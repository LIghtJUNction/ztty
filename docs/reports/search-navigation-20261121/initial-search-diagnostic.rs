extern crate alloc;
#[allow(dead_code)] mod before;
#[allow(dead_code)] mod after;
#[allow(dead_code)] mod hinted;
use std::{hint::black_box,time::Instant};
use zterm::Terminal;
fn measure(t:&Terminal,q:&str,insensitive:bool,variant:usize)->u128 {
 let start=Instant::now();
 for _ in 0..50 {
  match variant {
   0=>{black_box(before::find(black_box(t),black_box(q),if insensitive{before::Case::Insensitive}else{before::Case::Sensitive},black_box(None),black_box(false)));},
   1=>{black_box(after::find(black_box(t),black_box(q),if insensitive{after::Case::Insensitive}else{after::Case::Sensitive},black_box(None),black_box(false)));},
   2=>{let mut c=Box::<after::Cache>::default();black_box(c.find(black_box(t),black_box(q),if insensitive{after::Case::Insensitive}else{after::Case::Sensitive},black_box(None),black_box(false)));},
   3=>{let mut c=Box::<hinted::Cache>::default();black_box(c.find(black_box(t),black_box(q),if insensitive{hinted::Case::Insensitive}else{hinted::Case::Sensitive},black_box(None),black_box(false)));},
   _=>unreachable!()
  }
 }
 start.elapsed().as_nanos()
}
fn main(){
 let scenarios=[
 ("sparse_ascii","needle",false,80,"0123456789".repeat(7),format!("{}needle{}","x".repeat(30),"x".repeat(30)),73),
 ("dense_ascii","abab",false,80,"ab".repeat(40),"ab".repeat(40),1),
 ("unicode_lowercase_soft_wrap","ai\u{307}中e\u{301}",true,40,"ZzΩ中".repeat(10),format!("{}Aİ中e\u{301}\u{301}{}","z".repeat(38),"z".repeat(10)),7),
 ("no_match_ascii","needle",false,80,"0123456789".repeat(7),"0123456789".repeat(7),1)];
 println!("[");
 for (i,(name,q,insensitive,columns,noise,hit,interval)) in scenarios.iter().enumerate(){
  let mut t=Terminal::new(*columns,24,36064);
  for line in 0..12000 {t.feed(if line%interval==0{hit.as_bytes()}else{noise.as_bytes()});t.feed(b"\r\n");}
  for variant in 0..4 {black_box(measure(&t,q,*insensitive,variant));}
  let mut samples:[Vec<u128>;4]=std::array::from_fn(|_|Vec::new());
  for sample in 0..7 {for offset in 0..4 {let v=(sample+offset)%4;samples[v].push(measure(&t,q,*insensitive,v));}}
  println!("{{\"scenario\":\"{name}\",\"iterations\":50,\"samples_ns\":{:?}}}{}",samples,if i+1==scenarios.len(){""}else{","});
 }
 println!("]");
}
