fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("failed to locate vendored protoc");
    // Ensure prost-build uses the bundled protoc binary even if it's not installed system-wide.
    std::env::set_var("PROTOC", protoc);
    prost_build::compile_protos(&["src/proto/message.proto"], &["src"]).unwrap();
}
