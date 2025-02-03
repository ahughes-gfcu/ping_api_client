use std::{net::IpAddr, process::Command, time::{Duration, SystemTime}};
use ping_rs::PingReply;
use regex::Regex;
use reqwest::Client;
use tokio::task;
use uuid::Uuid;


#[macro_use]
extern crate ini;

fn gather_host_gateway() -> IpAddr {
    let output = if cfg!(target_os = "windows") {
        Command::new("ipconfig")
            .output()
            .expect("Failed to execute ipconfig")
    } else {
        Command::new("ip")
            .arg("route")
            .arg("show")
            .output()
            .expect("Failed to execute ip route show")
    };

    let output_str = String::from_utf8_lossy(&output.stdout);
    
    let gateway_regex = if cfg!(target_os = "windows") {
        Regex::new(r"Default Gateway[. ]+: ([\d.]+)").unwrap()
    } else {
        Regex::new(r"default via ([\d.]+)").unwrap()
    };
    
    let gateway = gateway_regex
        .captures_iter(&output_str)
        .next()
        .map(|cap| cap[1]
        .to_string());

    return gateway.unwrap_or_else(|| "Not found".to_string()).parse().unwrap();
}

fn read_endpoints() -> Vec<IpAddr> {
    let config = if cfg!(target_os = "windows") {
        ini!("C:\\ProgramData\\ping_api_client\\config.ini")
    } else {
        ini!("/etc/ping_api_client/config.ini")
    };
    let mut endpoints: Vec<IpAddr> = Vec::new();
    for value in config["endpoints"].clone().into_iter() {
        let endpoint_ip = value.1.unwrap();
        endpoints.push(endpoint_ip.parse().unwrap());
    }
    return endpoints;
}

async fn ping_endpoint(endpoint: IpAddr) -> Result<PingReply, ping_rs::PingError>{
    let data = [1,2,3,4];
    let options = ping_rs::PingOptions {
        ttl: 128,
        dont_fragment: true
    };

    let ping_time = ping_rs::send_ping(&endpoint, Duration::from_secs(10), &data, Some(&options));

    return ping_time;
}

#[tokio::main]
async fn main() {
    let gateway = gather_host_gateway();
    let endpoints = read_endpoints();

    let client = Client::new();
    let hostname = hostname::get().unwrap().into_string().unwrap();
    
    let mut handles = vec![];

    let config = if cfg!(target_os = "windows") {
        ini!("C:\\ProgramData\\ping_api_client\\config.ini")
    } else {
        ini!("/etc/ping_api_client/config.ini")
    };

    let prom_gateway_ip = config["pushgateway"]["ip"].clone().unwrap();
    let prom_gateway_port = config["pushgateway"]["port"].clone().unwrap();


    // Run pings concurrently for all endpoints including gateway
    for endpoint in std::iter::once(gateway).chain(endpoints) {
        let client_clone = client.clone();
        let hostname_clone = hostname.clone();
        let id = Uuid::new_v4();

        let prom_gateway = format!("http://{}:{}/metrics/job/{}", prom_gateway_ip, prom_gateway_port, id);

        handles.push(task::spawn(async move {
            loop {
                match ping_endpoint(endpoint).await {
                    Ok(ping_time) => {
                        let system_time = SystemTime::now();
                        let system_time = system_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis();
                        println!("{} || Ping time to {}: {}ms", system_time, endpoint, ping_time.rtt);

                        let prometheus_data = format!(
                            "# HELP ping_time Round Trip Time to Endpoint\n\
                             # TYPE ping_time gauge\n\
                            ping_time{{client=\"{}\", endpoint=\"{}\"}} {}
                            ", hostname_clone, endpoint, ping_time.rtt);

                        let _ = client_clone.post(&prom_gateway)
                            .body(prometheus_data)
                            .header("Content-Type", "text/plain")
                            .send()
                            .await;
                    },
                    Err(e) => {
                        let system_time = SystemTime::now();
                        let system_time = system_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis();
                        println!("{} || Error pinging {}: {:?}", system_time, endpoint, e);
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }));
    }

    for handle in handles {
        let _ =  handle.await;
    }
}
