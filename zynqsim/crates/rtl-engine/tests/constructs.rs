//! Behavioural constructs, system tasks, generate, snapshots and waveform output.

mod common;

use common::{build, diagnostics, try_build};
use rtl_engine::{Design, Simulator};

#[test]
fn case_casez_and_casex_select_the_first_matching_item() {
    // always_comb runs at time 0; `always @*` would wait for the first change of `sel`.
    let src = "module t;
        reg [3:0] sel = 4'b0100;
        reg [7:0] out1, out2, out3;
        always_comb begin
            case (sel)
                4'd1, 4'd2: out1 = 1;
                4'b0100:    out1 = 4;
                default:    out1 = 9;
            endcase
            casez (sel)
                4'b1???: out2 = 1;
                4'b01??: out2 = 2;
                default: out2 = 3;
            endcase
            casex (sel)
                4'b0x0x: out3 = 7;
                default: out3 = 8;
            endcase
        end
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(
        (s.get("out1"), s.get("out2"), s.get("out3")),
        (Some(4), Some(2), Some(7))
    );
    s.set("sel", 0b1001).unwrap();
    s.run_until(0).unwrap();
    assert_eq!(
        (s.get("out1"), s.get("out2"), s.get("out3")),
        (Some(9), Some(1), Some(8))
    );
}

#[test]
fn loops_break_and_continue_and_repeat_and_forever() {
    let src = "module t;
        integer sum = 0, i, n = 0, r = 0, k = 0;
        initial begin
            for (i = 1; i <= 10; i = i + 1) begin
                if (i == 3) continue;
                if (i == 8) break;
                sum = sum + i;
            end
            while (n < 5) n = n + 1;
            repeat (4) r = r + 2;
            forever begin
                k = k + 1;
                if (k == 6) disable_none;
                if (k > 5) break;
            end
        end
      endmodule";
    // `disable_none` is not a statement the engine knows: the design must be refused.
    let d = diagnostics("t", src);
    assert!(
        !d.is_empty(),
        "a forever loop with an unknown call is refused"
    );

    let src = "module t;
        integer sum = 0, i, n = 0, r = 0, k = 0;
        initial begin
            for (i = 1; i <= 10; i = i + 1) begin
                if (i == 3) continue;
                if (i == 8) break;
                sum = sum + i;
            end
            while (n < 5) n = n + 1;
            repeat (4) r = r + 2;
            forever begin
                k = k + 1;
                if (k > 5) break;
            end
        end
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    // 1+2+4+5+6+7 = 25
    assert_eq!(s.get("sum"), Some(25));
    assert_eq!(s.get("n"), Some(5));
    assert_eq!(s.get("r"), Some(8));
    assert_eq!(s.get("k"), Some(6));
}

#[test]
fn functions_return_values_and_take_arguments() {
    let src = "module t;
        function [7:0] inc(input [7:0] x);
            inc = x + 1;
        endfunction
        function signed [7:0] neg(input signed [7:0] x);
            begin neg = -x; end
        endfunction
        reg [7:0] a = 41;
        wire [7:0] b = inc(a);
        wire [7:0] c = neg(8'sd5);
        endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("b"), Some(42));
    assert_eq!(s.get("c"), Some(0xFB));
}

#[test]
fn recursive_functions_are_refused() {
    let d = diagnostics(
        "t",
        "module t; function [7:0] f(input [7:0] x); f = f(x - 1); endfunction wire [7:0] y = f(3); endmodule",
    );
    assert!(d.iter().any(|m| m.contains("recursive")), "{d:?}");
}

#[test]
fn tasks_with_delays_and_output_arguments_run_inline() {
    let src = "`timescale 1ns / 1ps
        module t;
        reg [7:0] got = 0;
        reg done = 0;
        task wait_and_double(input [7:0] x, output [7:0] y);
            begin
                #10;
                y = x << 1;
            end
        endtask
        initial begin
            wait_and_double(8'd21, got);
            done = 1;
        end
      endmodule";
    let mut s = build("t", src);
    s.run_until(9_999).unwrap();
    assert_eq!(s.get("done"), Some(0));
    s.run_until(10_000).unwrap();
    assert_eq!(s.get("got"), Some(42));
    assert_eq!(s.get("done"), Some(1));
}

#[test]
fn readmemh_loads_a_memory_from_a_file() {
    let dir = std::env::temp_dir().join(format!("rtl-engine-readmem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("init.hex"),
        "// two words and an address\nA5\n@2\nFF // third\n",
    )
    .unwrap();
    let src = "module t;
        reg [7:0] mem [0:3];
        wire [7:0] w0 = mem[0];
        wire [7:0] w2 = mem[2];
        initial $readmemh(\"init.hex\", mem);
      endmodule";
    let parsed =
        rtl_frontend::parser::parse_text("t.v", src, &rtl_frontend::parser::Options::default());
    let design = Design::elaborate_parsed("t", &parsed).with_base_dir(&dir);
    let mut s = Simulator::new(design).unwrap();
    s.run_until(0).unwrap();
    assert_eq!(s.get("w0"), Some(0xA5));
    assert_eq!(s.get("w2"), Some(0xFF));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn display_formats_match_verilog_padding() {
    let src = "module t;
        reg [7:0] v = 8'd5;
        reg [15:0] h = 16'hBEEF;
        reg [3:0] b = 4'b1010;
        initial begin
            $display(\"d=%0d d8=%d h=%h b=%b s=%s %%\", v, v, h, b, \"ok\");
            $display(\"m=%m\");
            $write(\"no newline \");
            $display(\"x%0d\", 8'd200);
        end
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    let log = s.log().to_vec();
    // %d of an 8-bit value pads to the 3 digits of 255; %h of 16 bits has 4 digits.
    assert_eq!(log[0], "d=5 d8=  5 h=beef b=1010 s=ok %");
    assert_eq!(log[1], "m=t");
    assert_eq!(log[2], "no newline x200");
}

#[test]
fn finish_stops_the_simulation_at_once() {
    let src = "`timescale 1ns / 1ps
        module t;
        integer ticks = 0;
        always #5 ticks = ticks + 1;
        initial begin #23 $display(\"stop\"); $finish; end
      endmodule";
    let mut s = build("t", src);
    s.run_until(1_000_000).unwrap();
    assert!(s.finished());
    assert_eq!(s.now_ps(), 23_000);
    assert_eq!(s.get("ticks"), Some(4));
    assert_eq!(s.log(), ["stop".to_string()]);
}

#[test]
fn parameters_generate_and_hierarchical_names() {
    let src = "module cell #(parameter W = 4) (input [W-1:0] i, output [W-1:0] o);
        assign o = ~i;
      endmodule
      module t;
        localparam N = 3;
        reg [7:0] din = 8'h0F;
        wire [7:0] dout [0:N-1];
        genvar g;
        generate
            for (g = 0; g < N; g = g + 1) begin : cells
                cell #(.W(8)) u (.i(din + g), .o(dout[g]));
            end
        endgenerate
        wire [7:0] first = dout[0];
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("first"), Some(0xF0));
    assert!(
        s.signal_names().iter().any(|n| n == "cells[2].u.o"),
        "{:?}",
        s.signal_names()
    );
    assert_eq!(s.get("cells[2].u.o"), Some(!(0x0Fu128 + 2) & 0xFF));
}

#[test]
fn a_port_connection_with_a_missing_module_is_refused_by_name() {
    let d = diagnostics("t", "module t; missing_mod u (.a(1'b0)); endmodule");
    assert!(d.iter().any(|m| m.contains("missing_mod")), "{d:?}");
}

#[test]
fn unsupported_system_tasks_are_refused_at_elaboration() {
    let d = diagnostics("t", "module t; reg [7:0] r; initial r = $random; endmodule");
    assert!(d.iter().any(|m| m.contains("$random")), "{d:?}");
}

#[test]
fn set_and_get_name_only_visible_signals_and_memories_differently() {
    let mut s = build("t", "module t; reg [7:0] a; reg [7:0] m [0:1]; endmodule");
    assert!(s.set("nope", 1).is_err());
    assert_eq!(s.get("nope"), None);
    assert!(s.set("m", 1).is_err(), "a memory is set word by word");
    s.set_word("m", 1, 0x7F).unwrap();
    assert_eq!(s.get_word("m", 1), Some(0x7F));
    assert_eq!(s.get_word("m", 2), None);
}

const SNAP_SRC: &str = "`timescale 1ns / 1ps
    module t;
    reg clk = 0; reg [15:0] cnt = 0; reg [15:0] acc = 0;
    always #5 clk = ~clk;
    always @(posedge clk) begin cnt <= cnt + 1; acc <= acc + cnt; end
    endmodule";

#[test]
fn snapshot_and_restore_reproduce_the_same_future() {
    let mut s = build("t", SNAP_SRC);
    s.run_until(100_000).unwrap();
    let snap = s.snapshot();
    let hash_at_snap = s.state_hash();
    s.run_until(300_000).unwrap();
    let (cnt_a, acc_a, hash_a) = (s.get("cnt"), s.get("acc"), s.state_hash());

    s.restore(&snap).unwrap();
    assert_eq!(s.state_hash(), hash_at_snap);
    s.run_until(300_000).unwrap();
    assert_eq!(
        (s.get("cnt"), s.get("acc"), s.state_hash()),
        (cnt_a, acc_a, hash_a)
    );
    assert!(cnt_a.unwrap() > 10);
}

#[test]
fn snapshot_rejects_another_design_and_garbage() {
    let s = build("t", SNAP_SRC);
    let snap = s.snapshot();
    let mut other = build("t", "module t; reg x; endmodule");
    assert!(other.restore(&snap).is_err());
    let mut same = build("t", SNAP_SRC);
    assert!(same.restore(b"nonsense").is_err());
    assert!(same.restore(&snap[..snap.len() - 1]).is_err());
}

#[test]
fn identical_runs_give_identical_state_hashes() {
    let mut hashes = Vec::new();
    for _ in 0..5 {
        let mut s = build("t", SNAP_SRC);
        s.run_until(250_000).unwrap();
        hashes.push(s.state_hash());
    }
    assert!(hashes.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn the_value_change_recorder_writes_a_vcd() {
    let mut s = build("t", SNAP_SRC);
    s.set_recording(true);
    s.run_until(30_000).unwrap();
    assert!(!s.changes().is_empty());
    let vcd = s.vcd();
    assert!(
        vcd.starts_with("$timescale"),
        "{}",
        &vcd[..40.min(vcd.len())]
    );
    assert!(vcd.contains("$var wire 16"), "{vcd}");
    assert!(vcd.contains("$enddefinitions"));
    assert!(vcd.lines().any(|l| l.starts_with('#') && l.len() > 1));
}

#[test]
fn tick_clock_is_one_edge_per_call() {
    let src = "module t; reg clk = 0; reg [7:0] n = 0; always @(posedge clk) n <= n + 1; endmodule";
    let mut s = build("t", src);
    s.tick_clock("clk").unwrap();
    s.tick_clock("clk").unwrap();
    assert_eq!(s.get("n"), Some(2));
    assert_eq!(s.now_ps(), 0, "tick_clock does not move time");
}

#[test]
fn a_design_with_diagnostics_is_not_simulated() {
    let r = try_build("t", "module t; reg a; wire b = c; endmodule");
    let err = r.err().expect("undeclared identifier refused");
    assert!(err.iter().any(|m| m.contains("`c`")), "{err:?}");
}
