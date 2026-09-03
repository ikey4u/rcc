fn main() {
    let id = capng::name_to_capability("net_admin").expect("capng_name_to_capability");
    println!("libcap-ng-ok cap_net_admin={id}");
}
