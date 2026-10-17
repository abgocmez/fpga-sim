`timescale 1ns / 1ps
// cosim_top: stands in for the PS7. A QEMU remote-port client (DPI-C, rp_dpi.c)
// turns each PS access to M_AXI_GP0 into AXI4-Lite beats on my_math_ip.
// Clock/reset match FCLK_CLK0 = 100 MHz.
module cosim_top;
`ifdef ZS_ICARUS
    // Icarus Verilog has no DPI-C (ADR-037): these are the VPI system calls of zynqsim.vpi.
    // `rp_next` is called as `$zs_rp_next` below (its outputs are arguments of the call);
    // the time goes through `zs_now`, since a `$time` argument reads as 0 in vvp.
    longint zs_now;
    function int rp_open(input string descr);
        rp_open = $zs_rp_open(descr);
    endfunction
    task rp_done(input int rdata, input int ok);
        $zs_rp_done(rdata, ok);
    endtask
    task rp_close();
        $zs_rp_close();
    endtask
`else
    import "DPI-C" function int  rp_open(input string descr);
    import "DPI-C" function int  rp_next(input longint now_ns, input int idle_ms,
                                         output int addr, output int wdata,
                                         output int wstrb, output int is_write);
    import "DPI-C" function void rp_done(input int rdata, input int ok);
    import "DPI-C" function void rp_close();
`endif

    reg aclk    = 1'b0;
    reg aresetn = 1'b0;
    always #5 aclk = ~aclk;

    reg  [3:0]  awaddr = 0, araddr = 0;
    reg         awvalid = 0, wvalid = 0, bready = 0, arvalid = 0, rready = 0;
    reg  [31:0] wdata = 0;
    reg  [3:0]  wstrb = 0;
    wire        awready, wready, bvalid, arready, rvalid;
    wire [1:0]  bresp, rresp;
    wire [31:0] rdata;

    my_math_ip dut (
        .S_AXI_ACLK(aclk), .S_AXI_ARESETN(aresetn),
        .S_AXI_AWADDR(awaddr), .S_AXI_AWPROT(3'b000),
        .S_AXI_AWVALID(awvalid), .S_AXI_AWREADY(awready),
        .S_AXI_WDATA(wdata), .S_AXI_WSTRB(wstrb),
        .S_AXI_WVALID(wvalid), .S_AXI_WREADY(wready),
        .S_AXI_BRESP(bresp), .S_AXI_BVALID(bvalid), .S_AXI_BREADY(bready),
        .S_AXI_ARADDR(araddr), .S_AXI_ARPROT(3'b000),
        .S_AXI_ARVALID(arvalid), .S_AXI_ARREADY(arready),
        .S_AXI_RDATA(rdata), .S_AXI_RRESP(rresp),
        .S_AXI_RVALID(rvalid), .S_AXI_RREADY(rready)
    );

    task axi_write(input [3:0] a, input [31:0] d, input [3:0] s, output ok);
        begin
            @(posedge aclk);
            awaddr <= a; wdata <= d; wstrb <= s;
            awvalid <= 1'b1; wvalid <= 1'b1;
            @(posedge aclk);
            while (!(awready && wready)) @(posedge aclk);
            awvalid <= 1'b0; wvalid <= 1'b0; bready <= 1'b1;
            @(posedge aclk);
            while (!bvalid) @(posedge aclk);
            ok = (bresp == 2'b00);
            bready <= 1'b0;
        end
    endtask

    task axi_read(input [3:0] a, output [31:0] d, output ok);
        begin
            @(posedge aclk);
            araddr <= a; arvalid <= 1'b1;
            @(posedge aclk);
            while (!arready) @(posedge aclk);
            arvalid <= 1'b0; rready <= 1'b1;
            @(posedge aclk);
            while (!rvalid) @(posedge aclk);
            d = rdata; ok = (rresp == 2'b00);
            rready <= 1'b0;
        end
    endtask

    integer a_i, wd_i, ws_i, iw_i, st;
    reg [31:0] rd;
    reg        ok;

    initial begin
        repeat (20) @(posedge aclk);
        aresetn = 1'b1;
        repeat (5) @(posedge aclk);

        if (rp_open("") != 0) begin
            $display("cosim_top: rp_open failed (is RP_SOCKET set and QEMU running?)");
            $finish;
        end
        $display("cosim_top: remote-port connected");

        forever begin
            // 1 ms real-time wait for QEMU; if idle, let simulated time move.
`ifdef ZS_ICARUS
            zs_now = $time;
            st = $zs_rp_next(zs_now, 1, a_i, wd_i, ws_i, iw_i);
`else
            st = rp_next($time, 1, a_i, wd_i, ws_i, iw_i);
`endif
            if (st == 0) begin
                $display("cosim_top: peer closed, finishing at %0t", $time);
                rp_close();
                $finish;
            end else if (st == 2) begin
                repeat (100) @(posedge aclk);
            end else begin
                // Only the low offset bits reach my_math_ip (4-bit address).
                if (iw_i != 0) begin
                    axi_write(a_i[3:0], wd_i, ws_i[3:0], ok);
                    $display("cosim_top: WR @%08h <= %08h strb=%b ok=%0d", a_i, wd_i, ws_i[3:0], ok);
                    rp_done(0, ok);
                end else begin
                    axi_read(a_i[3:0], rd, ok);
                    $display("cosim_top: RD @%08h => %08h ok=%0d", a_i, rd, ok);
                    rp_done(rd, ok);
                end
            end
        end
    end
endmodule
