//! Scheduling: continuous assigns, non-blocking semantics, edges, delta cycles,
//! delays, and the combinational-loop guard.

mod common;

use common::build;

#[test]
fn continuous_assign_follows_its_input_at_time_zero_and_after_a_change() {
    let mut s = build(
        "t",
        "module t; reg [7:0] a; wire [7:0] y = a + 8'd1; endmodule",
    );
    s.run_until(0).unwrap();
    assert_eq!(s.get("y"), Some(1));
    s.set("a", 5).unwrap();
    s.run_until(0).unwrap();
    assert_eq!(s.get("y"), Some(6));
}

#[test]
fn nonblocking_swap_exchanges_the_values() {
    let src = "`timescale 1ps / 1ps
        module t;
        reg clk = 0; reg [3:0] a = 1; reg [3:0] b = 2;
        always #5 clk = ~clk;
        always @(posedge clk) begin a <= b; b <= a; end
      endmodule";
    let mut s = build("t", src);
    s.run_until(4).unwrap();
    assert_eq!((s.get("a"), s.get("b")), (Some(1), Some(2)));
    s.run_until(6).unwrap(); // first posedge at 5 ps
    assert_eq!((s.get("a"), s.get("b")), (Some(2), Some(1)));
    s.run_until(16).unwrap(); // second posedge at 15 ps
    assert_eq!((s.get("a"), s.get("b")), (Some(1), Some(2)));
}

#[test]
fn blocking_swap_loses_a_value() {
    let src = "`timescale 1ps / 1ps
        module t;
        reg clk = 0; reg [3:0] a = 1; reg [3:0] b = 2;
        always #5 clk = ~clk;
        always @(posedge clk) begin a = b; b = a; end
      endmodule";
    let mut s = build("t", src);
    s.run_until(6).unwrap();
    assert_eq!((s.get("a"), s.get("b")), (Some(2), Some(2)));
}

#[test]
fn counter_driven_by_tick_clock_counts_race_free() {
    let src = "module t;
        reg clk = 0; reg [7:0] cnt = 0; reg [7:0] prev = 0;
        always @(posedge clk) begin prev <= cnt; cnt <= cnt + 1; end
      endmodule";
    let mut s = build("t", src);
    for _ in 0..10 {
        s.tick_clock("clk").unwrap();
    }
    assert_eq!(s.get("cnt"), Some(10));
    assert_eq!(s.get("prev"), Some(9));
}

#[test]
fn delta_cycles_through_a_chain_of_assigns_settle() {
    let src = "module t;
        reg [3:0] a = 0;
        wire [3:0] b = a + 1;
        wire [3:0] c = b + 1;
        wire [3:0] d = c + 1;
        endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("d"), Some(3));
    s.set("a", 7).unwrap();
    s.run_until(0).unwrap();
    assert_eq!(s.get("d"), Some(10));
}

#[test]
fn negedge_triggers_only_on_a_falling_level() {
    let src = "module t;
        reg clk = 0; reg [1:0] n = 0;
        always @(negedge clk) n <= n + 1;
        endmodule";
    let mut s = build("t", src);
    s.set("clk", 1).unwrap();
    s.run_until(0).unwrap();
    assert_eq!(s.get("n"), Some(0));
    s.set("clk", 0).unwrap();
    s.run_until(0).unwrap();
    assert_eq!(s.get("n"), Some(1));
}

#[test]
fn delays_advance_time_in_the_module_time_unit() {
    let src = "`timescale 1ns / 1ps
        module t;
        reg done = 0;
        initial begin #3 done = 1; #4 done = 0; end
        endmodule";
    let mut s = build("t", src);
    s.run_until(2_999).unwrap();
    assert_eq!(s.get("done"), Some(0));
    s.run_until(3_000).unwrap();
    assert_eq!(s.get("done"), Some(1));
    s.run_until(7_000).unwrap();
    assert_eq!(s.get("done"), Some(0));
}

#[test]
fn a_combinational_loop_is_reported_not_hung() {
    // Time zero runs when the simulator is created, so the loop is reported there.
    let err = common::try_build("t", "module t; wire a; assign a = ~a; endmodule")
        .err()
        .expect("oscillation must be reported");
    assert!(err[0].contains("combinational loop"), "{err:?}");
}

#[test]
fn a_process_with_no_timing_control_is_stopped_by_the_budget() {
    let err = common::try_build("t", "module t; reg a; initial forever a = ~a; endmodule")
        .err()
        .expect("must stop");
    assert!(err[0].contains("step budget"), "{err:?}");
}

#[test]
fn finish_stops_the_run() {
    let src = "module t; reg x = 0; initial begin x = 1; $finish; end endmodule";
    let mut s = build("t", src);
    s.run_until(100).unwrap();
    assert!(s.finished());
    assert_eq!(s.get("x"), Some(1));
}

#[test]
fn always_comb_runs_at_time_zero_and_follows_its_inputs() {
    let src = "module t;
        reg [3:0] a = 4'd3; reg [3:0] y;
        always_comb y = a << 1;
        endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("y"), Some(6));
    s.set("a", 9).unwrap();
    s.run_until(0).unwrap();
    assert_eq!(s.get("y"), Some(2)); // 9 << 1 = 18, truncated to 4 bits
}
