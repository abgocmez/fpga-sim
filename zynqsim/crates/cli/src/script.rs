//! Parsing of `--at TIME=ACTION` scripted inputs.

use crate::{Failure, fail};
use sim_api::board::BoardDef;
use sim_api::{Input, Ps};

/// Time of a `tap` hold, matching the MCP `board_press_button` default.
const TAP_HOLD: Ps = Ps(50 * Ps::PER_MS);

/// Parses every `--at` argument and returns the inputs sorted by time.
/// Events at the same time keep their command-line order.
pub fn parse(specs: &[String], def: &BoardDef) -> Result<Vec<(Ps, Input)>, Failure> {
    let mut out: Vec<(Ps, Input)> = Vec::new();
    for s in specs {
        let (time, action) = s.split_once('=').ok_or_else(|| {
            fail(
                2,
                format!("--at `{s}`: expected TIME=ACTION, e.g. 10ms=sw0:on"),
            )
        })?;
        let t = Ps(sim_api::parse_duration(time)
            .map_err(|e| fail(2, format!("--at `{s}`: {}", e.message)))?);
        let bad = |why: String| fail(2, format!("--at `{s}`: {why}"));

        let (target, arg) = match action.split_once(':') {
            Some((a, b)) => (a, Some(b)),
            None => (action, None),
        };
        match target {
            "reset" => {
                let seed = match arg {
                    Some(v) => Some(
                        v.parse::<u64>()
                            .map_err(|_| bad(format!("seed `{v}` is not a number")))?,
                    ),
                    None => None,
                };
                out.push((t, Input::Reset { seed }));
            }
            t_name if def.switches.iter().any(|x| x.id == t_name) => {
                let index = def
                    .switches
                    .iter()
                    .position(|x| x.id == t_name)
                    .unwrap_or(0);
                let on = match arg {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err(bad("switch action must be `on` or `off`".into())),
                };
                out.push((t, Input::SetSwitch { index, on }));
            }
            b_name if def.buttons.iter().any(|x| x.id == b_name) => {
                let index = def.buttons.iter().position(|x| x.id == b_name).unwrap_or(0);
                match arg {
                    Some("down") => out.push((
                        t,
                        Input::SetButton {
                            index,
                            pressed: true,
                        },
                    )),
                    Some("up") => out.push((
                        t,
                        Input::SetButton {
                            index,
                            pressed: false,
                        },
                    )),
                    Some("tap") => {
                        out.push((
                            t,
                            Input::SetButton {
                                index,
                                pressed: true,
                            },
                        ));
                        out.push((
                            t.saturating_add(TAP_HOLD),
                            Input::SetButton {
                                index,
                                pressed: false,
                            },
                        ));
                    }
                    _ => return Err(bad("button action must be `down`, `up` or `tap`".into())),
                }
            }
            u if u == "uart0" || u == "uart1" => {
                let port = u8::from(u == "uart1");
                let text =
                    arg.ok_or_else(|| bad("uart action needs text, e.g. uart1:hello".into()))?;
                out.push((
                    t,
                    Input::UartWrite {
                        port,
                        data: unescape(text).into_bytes(),
                    },
                ));
            }
            other => {
                let ids: Vec<&str> = def
                    .switches
                    .iter()
                    .map(|x| x.id.as_str())
                    .chain(def.buttons.iter().map(|x| x.id.as_str()))
                    .collect();
                return Err(bad(format!(
                    "unknown target `{other}`; use one of {}, uart0, uart1 or reset",
                    ids.join(", ")
                )));
            }
        }
    }
    // Stable sort keeps command-line order for equal times; applying inputs
    // out of time order would be rejected by replay.
    out.sort_by_key(|(t, _)| *t);
    Ok(out)
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(specs: &[&str]) -> Result<Vec<(Ps, Input)>, Failure> {
        let specs: Vec<String> = specs.iter().map(ToString::to_string).collect();
        parse(&specs, &BoardDef::zybo_z7_20())
    }

    #[test]
    fn parses_every_action_and_sorts_by_time() {
        let v = p(&[
            "20ms=btn1:tap",
            "10ms=sw2:on",
            "5ms=uart1:hi\\r\\n",
            "1s=reset:9",
        ])
        .unwrap();
        let times: Vec<u64> = v.iter().map(|(t, _)| t.0 / Ps::PER_MS).collect();
        assert_eq!(times, vec![5, 10, 20, 70, 1000]);
        assert_eq!(
            v[0].1,
            Input::UartWrite {
                port: 1,
                data: b"hi\r\n".to_vec()
            }
        );
        assert_eq!(v[1].1, Input::SetSwitch { index: 2, on: true });
        assert_eq!(v[4].1, Input::Reset { seed: Some(9) });
    }

    #[test]
    fn rejects_malformed_specs_with_the_offending_text() {
        for (spec, want) in [
            ("sw0:on", "expected TIME=ACTION"),
            ("10=sw0:on", "invalid duration"),
            ("1ms=sw9:on", "unknown target `sw9`"),
            ("1ms=sw0:maybe", "`on` or `off`"),
            ("1ms=btn0:push", "`down`, `up` or `tap`"),
            ("1ms=uart1", "needs text"),
            ("1ms=reset:x", "not a number"),
        ] {
            let e = p(&[spec])
                .err()
                .unwrap_or_else(|| panic!("{spec} should fail"));
            assert!(e.message.contains(want), "{spec}: {}", e.message);
            assert!(e.message.contains(spec), "{spec}: {}", e.message);
            assert_eq!(e.code, 2);
        }
    }
}
