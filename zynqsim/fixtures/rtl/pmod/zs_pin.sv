`timescale 1ns / 1ps
// zs_pin: pin-group models for ZynqSim testbenches. A Pmod port (group 0..4 =
// JA..JE) or the RGB LEDs (group 6: bits 0..2 LD5 R,G,B and 3..5 LD6 R,G,B).
// `zynqsim init --ip` instantiates the equivalent logic itself; these modules
// are for hand-written testbenches.

// Reports every change of `pins` to the host, stamped with the simulation time.
module zs_pin_monitor #(parameter int GROUP = 0) (input wire [7:0] pins);
    import "DPI-C" function void zs_pin_out(input int group, input int levels, input longint now_ns);
    always @(pins) zs_pin_out(GROUP, pins, $time);
endmodule

// Drives `pins` from the levels the host sets (a Pmod button module...),
// sampled every microsecond.
module zs_pin_driver #(parameter int GROUP = 0) (output reg [7:0] pins);
    import "DPI-C" function int zs_pin_in(input int group);
    initial pins = 8'h00;
    always #1000 pins <= zs_pin_in(GROUP);
endmodule
