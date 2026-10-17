// X-propagation testbench for the differential test in tests/xprop_diff.rs.
// Every check prints one line `R <name> <value>` with %b, so the output of XSim and of
// the native engine can be compared line by line. Only standard Verilog-2001 is used.
`timescale 1ns / 1ps
module xprop_tb;
  reg  [3:0] a, b;
  reg        c;
  reg        clk, rst;
  reg  [3:0] q;
  reg        en1, en2;
  reg  [1:0] cz;
  reg        ev;
  reg  [3:0] n;

  wire [3:0] w_and  = a & b;
  wire [3:0] w_or   = a | b;
  wire [3:0] w_xor  = a ^ b;
  wire [3:0] w_xnor = a ~^ b;
  wire [3:0] w_not  = ~a;
  wire [3:0] w_add  = a + b;
  wire [3:0] w_mux  = c ? a : b;
  wire       w_eq   = a == b;
  wire       w_ne   = a != b;
  wire       w_ceq  = a === b;
  wire       w_cne  = a !== b;
  wire       w_lt   = a < b;
  wire       w_land = a && b;
  wire       w_lor  = a || b;
  wire       w_lnot = !a;
  wire       w_rand = &a;
  wire       w_ror  = |a;
  wire       w_rxor = ^a;
  wire [3:0] w_shl  = a << 1;
  wire [3:0] w_shr  = a >> 1;
  wire [3:0] w_shx  = b << a[0];
  wire [1:0] bus;
  assign bus = en1 ? 2'b10 : 2'bzz;
  assign bus = en2 ? 2'b01 : 2'bzz;

  // Counter without reset: X until the first reset.
  always @(posedge clk)
    if (rst) q <= 4'b0000;
    else q <= q + 4'd1;

  // casez with a z-bearing selector, and an if on an unknown.
  always @(a) begin
    casez (a)
      4'b1?0?: cz = 2'd1;
      default: cz = 2'd2;
    endcase
  end

  initial begin
    clk = 0; rst = 1'b0; c = 1'bx; en1 = 1'b0; en2 = 1'b0;
    a = 4'b1x0z; b = 4'b0101;
    #1;
    $display("R and %b", w_and);
    $display("R or %b", w_or);
    $display("R xor %b", w_xor);
    $display("R xnor %b", w_xnor);
    $display("R not %b", w_not);
    $display("R add %b", w_add);
    $display("R mux_x %b", w_mux);
    $display("R eq %b", w_eq);
    $display("R ne %b", w_ne);
    $display("R ceq %b", w_ceq);
    $display("R cne %b", w_cne);
    $display("R lt %b", w_lt);
    $display("R land %b", w_land);
    $display("R lor %b", w_lor);
    $display("R lnot %b", w_lnot);
    $display("R rand %b", w_rand);
    $display("R ror %b", w_ror);
    $display("R rxor %b", w_rxor);
    $display("R shl %b", w_shl);
    $display("R shr %b", w_shr);
    $display("R shx %b", w_shx);
    $display("R cz %b", cz);
    $display("R bus_none %b", bus);
    en1 = 1'b1; #1;
    $display("R bus_one %b", bus);
    en2 = 1'b1; #1;
    $display("R bus_conflict %b", bus);
    en1 = 1'b0; #1;
    $display("R bus_two %b", bus);
    en2 = 1'b0; #1;
    $display("R bus_zero %b", bus);

    c = 1'b1; a = 4'b1010; b = 4'b0110; #1;
    $display("R known_add %b", w_add);
    c = 1'bx; #1;
    $display("R mux_x_known %b", w_mux);

    // Clock the counter: unknown, unknown, then reset, then counting.
    rst = 1'b0; #1;
    clk = 1'b1; #1; clk = 1'b0; #1;
    $display("R count0 %b", q);
    clk = 1'b1; #1; clk = 1'b0; #1;
    $display("R count1 %b", q);
    rst = 1'b1; #1;
    clk = 1'b1; #1; clk = 1'b0; #1;
    $display("R reset %b", q);
    rst = 1'b0; #1;
    clk = 1'b1; #1; clk = 1'b0; #1;
    $display("R counted %b", q);

    // An if on an unknown takes the else branch.
    ev = 1'bx; n = 4'd0;
    if (ev) n = 4'd1; else n = 4'd2;
    $display("R if_x %b", n);
    $display("R ev %b", ev);
    $finish;
  end
endmodule
