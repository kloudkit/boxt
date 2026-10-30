#[test]
fn binds() {
    let _ = std::net::TcpListener::bind("127.0.0.1:0");
}
