// Written for the ZynqSim universality corpus (not third-party code).
class pkt;
  rand bit [7:0] a;
  rand bit [7:0] b;
  constraint c { a < b; a > 10; }
endclass
module top;
  pkt p;
  integer errors = 0;
  initial begin
    p = new();
    repeat (20) begin
      if (!p.randomize()) errors = errors + 1;
      else if (!(p.a < p.b && p.a > 10)) errors = errors + 1;
    end
    if (errors == 0) $display("CORPUS_PASS probe-constrained-random");
    else $display("CORPUS_FAIL probe-constrained-random");
    $finish;
  end
endmodule
