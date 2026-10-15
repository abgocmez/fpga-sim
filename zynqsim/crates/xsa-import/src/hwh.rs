//! The HWH reader.

use crate::catalog::{IpClass, classify};
use crate::model::*;
use roxmltree::{Document, Node};
use std::collections::BTreeMap;

fn num(s: &str) -> Option<u64> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

fn elems<'a, 'b: 'a>(n: Node<'a, 'b>, tag: &'static str) -> impl Iterator<Item = Node<'a, 'b>> {
    n.children()
        .filter(move |c| c.is_element() && c.has_tag_name(tag))
}

fn child<'a, 'b: 'a>(n: Node<'a, 'b>, tag: &'static str) -> Option<Node<'a, 'b>> {
    elems(n, tag).next()
}

fn attr(n: Node<'_, '_>, name: &str) -> Option<String> {
    n.attribute(name).map(str::to_string)
}

fn parse_port(p: Node<'_, '_>) -> Port {
    let left = p.attribute("LEFT").and_then(num);
    let right = p.attribute("RIGHT").and_then(num);
    let width = match (left, right) {
        (Some(l), Some(r)) => (l.max(r) - l.min(r) + 1) as u32,
        _ => 1,
    };
    let connections = child(p, "CONNECTIONS")
        .map(|c| {
            elems(c, "CONNECTION")
                .filter_map(|x| {
                    Some(PortConnection {
                        instance: attr(x, "INSTANCE")?,
                        port: attr(x, "PORT")?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Port {
        name: attr(p, "NAME").unwrap_or_default(),
        dir: attr(p, "DIR").unwrap_or_default(),
        width,
        sigis: attr(p, "SIGIS"),
        sensitivity: attr(p, "SENSITIVITY"),
        clk_hz: p.attribute("CLKFREQUENCY").and_then(num),
        polarity: attr(p, "POLARITY"),
        connections,
    }
}

fn parse_interfaces(m: Node<'_, '_>) -> Vec<BusInterface> {
    let Some(list) = child(m, "BUSINTERFACES") else {
        return Vec::new();
    };
    elems(list, "BUSINTERFACE")
        .map(|b| {
            let protocol = elems(b, "PARAMETER")
                .find(|p| p.attribute("NAME") == Some("PROTOCOL"))
                .and_then(|p| attr(p, "VALUE"));
            BusInterface {
                name: attr(b, "NAME").unwrap_or_default(),
                kind: attr(b, "TYPE").unwrap_or_default(),
                protocol,
                data_width: b.attribute("DATAWIDTH").and_then(num).map(|v| v as u32),
                bus_name: attr(b, "BUSNAME"),
                vlnv: attr(b, "VLNV"),
            }
        })
        .collect()
}

fn parse_params(m: Node<'_, '_>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(list) = child(m, "PARAMETERS") {
        for p in elems(list, "PARAMETER") {
            if let Some(n) = attr(p, "NAME") {
                out.insert(n, attr(p, "VALUE").unwrap_or_default());
            }
        }
    }
    out
}

fn parse_memory_map(m: Node<'_, '_>, owner: &str) -> Vec<AddrRange> {
    let Some(map) = child(m, "MEMORYMAP") else {
        return Vec::new();
    };
    elems(map, "MEMRANGE")
        .filter_map(|r| {
            Some(AddrRange {
                master: owner.to_string(),
                master_interface: attr(r, "MASTERBUSINTERFACE").unwrap_or_default(),
                instance: attr(r, "INSTANCE")?,
                interface: attr(r, "SLAVEBUSINTERFACE").unwrap_or_default(),
                block: attr(r, "ADDRESSBLOCK").unwrap_or_default(),
                base: r.attribute("BASEVALUE").and_then(num)?,
                high: r.attribute("HIGHVALUE").and_then(num)?,
                mem_type: attr(r, "MEMTYPE"),
            })
        })
        .collect()
}

fn flag(params: &BTreeMap<String, String>, name: &str) -> bool {
    params
        .get(name)
        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "True"))
}

fn float(params: &BTreeMap<String, String>, name: &str) -> Option<f64> {
    params.get(name).and_then(|v| v.trim().parse().ok())
}

fn is_select(v: &str) -> bool {
    let t = v.trim();
    t.is_empty() || t == "<Select>"
}

fn parse_ps7(
    inst: &str,
    vlnv: &str,
    params: &BTreeMap<String, String>,
    ports: &[Port],
    ifaces: &[BusInterface],
) -> Ps7 {
    let port = |n: &str| ports.iter().find(|p| p.name == n);
    let mut fclk: [FclkInfo; 4] = Default::default();
    for (i, f) in fclk.iter_mut().enumerate() {
        let p = port(&format!("FCLK_CLK{i}"));
        f.enabled = flag(params, &format!("PCW_EN_CLK{i}_PORT")) || p.is_some();
        f.requested_mhz = float(params, &format!("PCW_FPGA{i}_PERIPHERAL_FREQMHZ"));
        f.actual_mhz = float(params, &format!("PCW_ACT_FPGA{i}_PERIPHERAL_FREQMHZ"));
        f.port_hz = p.and_then(|p| p.clk_hz);
        f.reset_enabled = flag(params, &format!("PCW_EN_RST{i}_PORT"))
            || port(&format!("FCLK_RESET{i}_N")).is_some();
    }
    let has_if = |n: &str| ifaces.iter().any(|b| b.name == n);
    let use_flag = |n: &str| flag(params, &format!("PCW_USE_{n}")) || has_if(n);
    let irq_port = port("IRQ_F2P");
    let irq_enabled = flag(params, "PCW_USE_FABRIC_INTERRUPT") && flag(params, "PCW_IRQ_F2P_INTR")
        || (irq_port.is_some() && flag(params, "PCW_USE_FABRIC_INTERRUPT"));
    let width = irq_port
        .map(|p| p.width)
        .or_else(|| {
            params
                .get("PCW_NUM_F2P_INTR_INPUTS")
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0);

    // MIO use from the pin tree.
    let tree = |name: &str| -> Vec<String> {
        params
            .get(name)
            .map(|v| v.split('#').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default()
    };
    let (pers, sigs) = (
        tree("PCW_MIO_TREE_PERIPHERALS"),
        tree("PCW_MIO_TREE_SIGNALS"),
    );
    let mio = if pers.len() == sigs.len() && !pers.is_empty() {
        pers.into_iter()
            .zip(sigs)
            .enumerate()
            .map(|(i, (peripheral, signal))| MioPin {
                index: i as u32,
                peripheral,
                signal,
            })
            .collect()
    } else {
        Vec::new()
    };

    // Peripheral enables: `PCW_<P>_PERIPHERAL_ENABLE` or `PCW_EN_<P>`.
    let mut peripherals = Vec::new();
    for (name, io_param) in [
        ("UART0", "PCW_UART0_UART0_IO"),
        ("UART1", "PCW_UART1_UART1_IO"),
        ("ENET0", "PCW_ENET0_ENET0_IO"),
        ("ENET1", "PCW_ENET1_ENET1_IO"),
        ("SD0", "PCW_SD0_SD0_IO"),
        ("SD1", "PCW_SD1_SD1_IO"),
        ("USB0", "PCW_USB0_USB0_IO"),
        ("USB1", "PCW_USB1_USB1_IO"),
        ("QSPI", "PCW_QSPI_QSPI_IO"),
        ("I2C0", "PCW_I2C0_I2C0_IO"),
        ("I2C1", "PCW_I2C1_I2C1_IO"),
        ("SPI0", "PCW_SPI0_SPI0_IO"),
        ("SPI1", "PCW_SPI1_SPI1_IO"),
        ("CAN0", "PCW_CAN0_CAN0_IO"),
        ("CAN1", "PCW_CAN1_CAN1_IO"),
        ("TTC0", "PCW_TTC0_TTC0_IO"),
        ("TTC1", "PCW_TTC1_TTC1_IO"),
        ("WDT", "PCW_WDT_WDT_IO"),
    ] {
        let en_name = match name {
            "SD0" => "SDIO0".to_string(),
            "SD1" => "SDIO1".to_string(),
            n => n.to_string(),
        };
        let enabled = flag(params, &format!("PCW_{name}_PERIPHERAL_ENABLE"))
            || flag(params, &format!("PCW_EN_{en_name}"));
        let io = params
            .get(io_param)
            .filter(|v| !is_select(v))
            .cloned()
            .filter(|_| enabled);
        peripherals.push(PsPeripheral {
            name: name.to_string(),
            enabled,
            io,
        });
    }
    let gpio_mio = flag(params, "PCW_GPIO_MIO_GPIO_ENABLE") || flag(params, "PCW_EN_GPIO");
    peripherals.push(PsPeripheral {
        name: "GPIO".to_string(),
        enabled: gpio_mio || flag(params, "PCW_GPIO_EMIO_GPIO_ENABLE"),
        io: if flag(params, "PCW_GPIO_EMIO_GPIO_ENABLE") {
            Some(if gpio_mio { "MIO + EMIO" } else { "EMIO" }.to_string())
        } else if gpio_mio {
            Some("MIO".to_string())
        } else {
            None
        },
    });
    let emio_gpio_width = flag(params, "PCW_GPIO_EMIO_GPIO_ENABLE")
        .then(|| {
            params
                .get("PCW_GPIO_EMIO_GPIO_WIDTH")
                .and_then(|v| v.trim().parse().ok())
        })
        .flatten();

    Ps7 {
        instance: inst.to_string(),
        vlnv: vlnv.to_string(),
        fclk,
        m_axi_gp: [use_flag("M_AXI_GP0"), use_flag("M_AXI_GP1")],
        s_axi_gp: [use_flag("S_AXI_GP0"), use_flag("S_AXI_GP1")],
        s_axi_hp: [
            use_flag("S_AXI_HP0"),
            use_flag("S_AXI_HP1"),
            use_flag("S_AXI_HP2"),
            use_flag("S_AXI_HP3"),
        ],
        s_axi_acp: use_flag("S_AXI_ACP"),
        irq_f2p_enabled: irq_enabled,
        irq_f2p_width: width,
        emio_gpio_width,
        mio,
        peripherals,
        param_count: params.len(),
    }
}

/// Parses HWH text.
pub fn parse_hwh(text: &str) -> Result<Design, String> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = Document::parse(text).map_err(|e| format!("not valid XML: {e}"))?;
    let root = doc.root_element();
    if !root.has_tag_name("EDKSYSTEM") {
        return Err(format!(
            "root element is <{}>, expected <EDKSYSTEM> (is this a hardware handoff?)",
            root.tag_name().name()
        ));
    }
    let sys = child(root, "SYSTEMINFO");
    let sa = |n: &str| sys.and_then(|s| attr(s, n));
    let mut design = Design {
        name: sa("NAME").unwrap_or_default(),
        vivado_version: attr(root, "VIVADOVERSION"),
        timestamp: attr(root, "TIMESTAMP"),
        arch: sa("ARCH"),
        device: sa("DEVICE"),
        package: sa("PACKAGE"),
        speed_grade: sa("SPEEDGRADE"),
        board: sa("BOARD"),
        hwh_file: None,
        ps7: None,
        ips: Vec::new(),
        memory_map: Vec::new(),
        interrupts: Vec::new(),
        clocks: Vec::new(),
        resets: Vec::new(),
        notes: Vec::new(),
    };

    let Some(modules) = child(root, "MODULES") else {
        return Err("no <MODULES> element: the design has no IP instances".to_string());
    };
    for m in elems(modules, "MODULE") {
        let instance = attr(m, "INSTANCE").unwrap_or_default();
        let vlnv = attr(m, "VLNV").unwrap_or_default();
        let modtype = attr(m, "MODTYPE").unwrap_or_default();
        let params = parse_params(m);
        let ports: Vec<Port> = child(m, "PORTS")
            .map(|p| elems(p, "PORT").map(parse_port).collect())
            .unwrap_or_default();
        let interfaces = parse_interfaces(m);
        let class = classify(&vlnv, &modtype);
        let ranges = parse_memory_map(m, &instance);
        design.memory_map.extend(ranges.iter().cloned());
        if class == IpClass::Ps7 {
            if design.ps7.is_some() {
                design
                    .notes
                    .push(format!("second PS7 instance `{instance}` ignored"));
                continue;
            }
            design.ps7 = Some(parse_ps7(&instance, &vlnv, &params, &ports, &interfaces));
            continue;
        }
        design.ips.push(IpInstance {
            full_name: attr(m, "FULLNAME").unwrap_or_else(|| format!("/{instance}")),
            instance,
            vlnv,
            modtype,
            class,
            params,
            interfaces,
            ports,
            address: Vec::new(),
            master_windows: ranges,
        });
    }

    // Address windows: a MEMRANGE of the PS7 is a window into a PL slave.
    let ps_name = design.ps7.as_ref().map(|p| p.instance.clone());
    for r in design.memory_map.clone() {
        if Some(&r.master) == ps_name.as_ref() {
            if let Some(ip) = design.ips.iter_mut().find(|i| i.instance == r.instance) {
                ip.address.push(r);
            }
        }
    }
    // Windows reached by PL masters stay in master_windows; drop the PS7 ones that were
    // assigned (the PS7 is not an IpInstance, so none were stored there).

    resolve_interrupts(&mut design);
    resolve_clocks_and_resets(&mut design);
    Ok(design)
}

fn is_input_name(name: &str) -> Option<u32> {
    let l = name.to_ascii_lowercase();
    l.strip_prefix("in")?.parse().ok()
}

fn resolve_interrupts(design: &mut Design) {
    let Some(ps) = design.ps7.as_ref() else {
        return;
    };
    let ps_name = ps.instance.clone();
    // The PS7's IRQ_F2P connections are in its own PORTS, which the parser did not keep on a
    // `Ps7`; the sources list the PS7 as their sink, so scan the PL modules' output ports.
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let sinks: Vec<(String, String)> = design
        .ips
        .iter()
        .flat_map(|ip| {
            ip.ports
                .iter()
                .filter(|p| p.dir == "O")
                .filter(|p| {
                    p.connections
                        .iter()
                        .any(|c| c.instance == ps_name && c.port == "IRQ_F2P")
                })
                .map(|p| (ip.instance.clone(), p.name.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    for (inst, port) in sinks {
        trace_irq(
            design,
            &inst,
            &port,
            0,
            &mut Vec::new(),
            &mut out,
            &mut notes,
            0,
        );
    }
    out.sort_by(|a: &IrqLink, b| a.bit.cmp(&b.bit).then(a.instance.cmp(&b.instance)));
    design.interrupts = out;
    design.notes.extend(notes);
}

#[allow(clippy::too_many_arguments)]
fn trace_irq(
    design: &Design,
    inst: &str,
    port: &str,
    offset: u32,
    via: &mut Vec<String>,
    out: &mut Vec<IrqLink>,
    notes: &mut Vec<String>,
    depth: u32,
) {
    if depth > 8 {
        notes.push(format!(
            "interrupt trace through `{inst}` is too deep; stopped"
        ));
        return;
    }
    let Some(ip) = design.ip(inst) else {
        notes.push(format!(
            "interrupt source `{inst}.{port}` is not a module of this design"
        ));
        return;
    };
    if ip.modtype == "xlconcat" {
        // The concat's dout feeds the sink: its In<k> inputs are consecutive slices.
        let mut ins: Vec<(u32, &Port)> = ip
            .ports
            .iter()
            .filter(|p| p.dir == "I")
            .filter_map(|p| is_input_name(&p.name).map(|k| (k, p)))
            .collect();
        ins.sort_by_key(|(k, _)| *k);
        let mut off = offset;
        via.push(inst.to_string());
        for (_, p) in ins {
            for c in &p.connections {
                trace_irq(
                    design,
                    &c.instance,
                    &c.port,
                    off,
                    via,
                    out,
                    notes,
                    depth + 1,
                );
            }
            off += p.width;
        }
        via.pop();
        return;
    }
    let p = ip.ports.iter().find(|p| p.name == port);
    out.push(IrqLink {
        instance: inst.to_string(),
        port: port.to_string(),
        bit: offset,
        width: p.map_or(1, |p| p.width),
        sensitivity: p.and_then(|p| p.sensitivity.clone()),
        via: via.clone(),
    });
}

fn resolve_clocks_and_resets(design: &mut Design) {
    let ps_name = design.ps7.as_ref().map(|p| p.instance.clone());
    let mut clocks = Vec::new();
    let mut resets = Vec::new();
    for ip in &design.ips {
        for p in &ip.ports {
            if p.dir != "I" {
                continue;
            }
            let src = p.connections.first();
            let from_ps = |prefix: &str| -> Option<u8> {
                let c = src?;
                if Some(&c.instance) != ps_name.as_ref() {
                    return None;
                }
                c.port.strip_prefix(prefix)?.split('_').next()?.parse().ok()
            };
            match p.sigis.as_deref() {
                Some("clk") => clocks.push(ClockLink {
                    instance: ip.instance.clone(),
                    port: p.name.clone(),
                    source_instance: src.map(|c| c.instance.clone()),
                    source_port: src.map(|c| c.port.clone()),
                    fclk: from_ps("FCLK_CLK"),
                    hz: p.clk_hz,
                }),
                Some("rst") => resets.push(ResetLink {
                    instance: ip.instance.clone(),
                    port: p.name.clone(),
                    source_instance: src.map(|c| c.instance.clone()),
                    source_port: src.map(|c| c.port.clone()),
                    fclk_reset: from_ps("FCLK_RESET"),
                }),
                _ => {}
            }
        }
    }
    design.clocks = clocks;
    design.resets = resets;
}
