//! Prints every declared request, one a line: method, path, access, name.

fn main() {
    for route in orch_api_contract::routes::all() {
        println!(
            "{}\t{}\t{:?}\t{}",
            route.method.as_str(),
            route.path,
            route.access,
            route.name
        );
    }
}
