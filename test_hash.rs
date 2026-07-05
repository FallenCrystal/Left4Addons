use md5::{Md5, Digest};

fn main() {
    let mut hasher = Md5::new();
    hasher.update(b"hello");
    let hash_result = hasher.finalize();
    println!("{:x}", hash_result);
}
