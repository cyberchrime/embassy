#![no_std]
#![no_main]

use defmt::{info, warn, unwrap};
use embassy_executor::Spawner;
use embassy_net::{Ipv4Cidr, Ipv4Address, Stack, StackResources};
use heapless::Vec;
use embassy_stm32::eth::generic_smi::GenericSMI;
use embassy_stm32::eth::{Ethernet, PacketQueue};
use embassy_stm32::peripherals::ETH;
use embassy_stm32::rng::Rng;
use embassy_stm32::{bind_interrupts, eth, peripherals, rng, Config};
use embassy_time::Duration;
use embedded_io_async::Write;
use embedded_io::Write as bWrite;
use rand_core::RngCore;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    ETH => eth::InterruptHandler;
    RNG => rng::InterruptHandler<peripherals::RNG>;
});

type Device = Ethernet<'static, ETH, GenericSMI>;

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<Device>) -> ! {
    stack.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    let mut config = Config::default();
    {
        use embassy_stm32::rcc::*;
        config.rcc.hsi = Some(HSIPrescaler::DIV1);
        config.rcc.csi = true;
        config.rcc.hsi48 = Some(Default::default()); // needed for RNG
        config.rcc.pll1 = Some(Pll {
            source: PllSource::HSI,
            prediv: PllPreDiv::DIV4,
            mul: PllMul::MUL50,
            divp: Some(PllDiv::DIV2),
            divq: None,
            divr: None,
        });
        config.rcc.sys = Sysclk::PLL1_P; // 400 Mhz
        config.rcc.ahb_pre = AHBPrescaler::DIV2; // 200 Mhz
        config.rcc.apb1_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.apb2_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.apb3_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.apb4_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.voltage_scale = VoltageScale::Scale1;
    }
    let p = embassy_stm32::init(config);
    info!("Hello World!");

    // Generate random seed.
    let mut rng = Rng::new(p.RNG, Irqs);
    let mut seed = [0; 8];
    rng.fill_bytes(&mut seed);
    let seed = u64::from_le_bytes(seed);

    let mac_addr = [0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF];

    static PACKETS: StaticCell<PacketQueue<4, 4>> = StaticCell::new();
    let device = Ethernet::new(
        PACKETS.init(PacketQueue::<4, 4>::new()),
        p.ETH,
        Irqs,
        p.PA1,
        p.PA2,
        p.PC1,
        p.PA7,
        p.PC4,
        p.PC5,
        p.PG13,
        p.PB13,
        p.PG11,
        GenericSMI::new(0),
        mac_addr,
    );

    //let config = embassy_net::Config::dhcpv4(Default::default());
    let config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(10, 42, 0, 61), 24),
        dns_servers: Vec::new(),
        gateway: Some(Ipv4Address::new(10, 42, 0, 1)),
    });

    // Init network stack
    static STACK: StaticCell<Stack<Device>> = StaticCell::new();
    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let stack = &*STACK.init(Stack::new(
        device,
        config,
        RESOURCES.init(StackResources::<3>::new()),
        seed,
    ));

    // Launch network task
    unwrap!(spawner.spawn(net_task(&stack)));

    // Ensure DHCP configuration is up before trying connect
    stack.wait_config_up().await;

    info!("Network task initialized");

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];
    let mut buf = [0; 4096];
    loop {
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        //led.set_low();
        info!("Listening on TCP:80...");
        if let Err(e) = socket.accept(80).await {
            warn!("accept error: {:?}", e);
            continue;
        }
        info!("Received connection from {:?}", socket.remote_endpoint());
        //led.set_high();

        loop {
            let n = match socket.read(&mut buf).await {
                Ok(0) => {
                    warn!("read EOF");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    warn!("{:?}", e);
                    break;
                }
            };
            info!("rxd {}", core::str::from_utf8(&buf[..n]).unwrap());

            let s = core::str::from_utf8(&buf[..n]).unwrap();
            match &s.split(" ").next() {
                Some("GET") => info!("GET"),
                Some("POST") => info!("POST: Toggle LED"),
                _ => {
                    warn!("Unexpected request: {:?}", s);
                    break;
                },
            }

            let status_line = "HTTP/1.1 200 OK";
            let contents = PAGE;
            let length = contents.len();

            let _ = write!(
                &mut buf[..],
                "{status_line}\r\nContent-Length: {length}\r\n\r\n{contents}\r\n\0"
            );

            if let Err(e) = socket.write_all(&buf).await {
                warn!("write error: {:?}", e);
                break;
            }
        }
    }
}

// Web page
const PAGE: &str = r#"<!DOCTYPE html>
<html>
<body>

<h1>The button Element</h1>

<form action="/">
  <input type="submit" formmethod="post" value="Submit">
</form> 

</body>
</html>"#;