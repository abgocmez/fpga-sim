`timescale 1ns / 1ps
// Testbench for fixtures/rtl/pmod/pmod_simple.v: stands in for the PS7 and drives
// `pmod_simple` through its AXI4-Lite slave `S_AXI_*` from the remote-port link (lock-step).
// Like `zynqsim init --ip` output, it reports the JA pins to the host (pin group 0, the
// 8LD) and samples the JB pins from the host's levels once a microsecond (pin group 1,
// the Pmod BTN). A write of 0xC051D0DE to offset 0x3C ends the run.
module cosim_top;
    import "DPI-C" function int  rp_open(input string descr);
    import "DPI-C" function int  rp_next(input longint now_ns, input int idle_ms,
                                          output int addr, output int wdata,
                                          output int wstrb, output int is_write);
    import "DPI-C" function void rp_done_t(input int rdata, input int ok, input longint now_ns);
    import "DPI-C" function void rp_close();
    import "DPI-C" function void rp_lockstep(input int on);
    import "DPI-C" function longint rp_sync_target();
    import "DPI-C" function void rp_sync_ack(input longint now_ns);
    import "DPI-C" function longint rp_req_ts();
    import "DPI-C" function void rp_irq(input longint now_ns, input int line, input int val);
    import "DPI-C" function void rp_mem_put(input int idx, input int word);
    import "DPI-C" function int  rp_mem_start(input longint now_ns, input int dev,
                                              input int is_write, input int addr, input int len);
    import "DPI-C" function int  rp_mem_wait(input int slot);
    import "DPI-C" function int  rp_mem_get(input int slot, input int idx);
    import "DPI-C" function void rp_mem_free(input int slot);
    import "DPI-C" function void zs_pin_out(input int group, input int levels, input longint now_ns);
    import "DPI-C" function int  zs_pin_in(input int group);

    reg aclk = 1'b0;
    reg aresetn = 1'b0;
    always #5 aclk = ~aclk;   // 100 MHz = FCLK_CLK0

    reg  [4:0] S_AXI_AWADDR = 0;
    reg  S_AXI_AWVALID = 0;
    wire S_AXI_AWREADY;
    reg  [31:0] S_AXI_WDATA = 0;
    reg  [3:0] S_AXI_WSTRB = 0;
    reg  S_AXI_WVALID = 0;
    wire S_AXI_WREADY;
    wire [1:0] S_AXI_BRESP;
    wire S_AXI_BVALID;
    reg  S_AXI_BREADY = 0;
    reg  [4:0] S_AXI_ARADDR = 0;
    reg  S_AXI_ARVALID = 0;
    wire S_AXI_ARREADY;
    wire [31:0] S_AXI_RDATA;
    wire [1:0] S_AXI_RRESP;
    wire S_AXI_RVALID;
    reg  S_AXI_RREADY = 0;
    wire [7:0] pmod_ja;
    reg  [7:0] pmod_jb_i = '1;     // driven by the host (released = 1 until the first sample)

    pmod_simple dut (
        .S_AXI_ACLK(aclk),
        .S_AXI_ARESETN(aresetn),
        .S_AXI_AWADDR(S_AXI_AWADDR),
        .S_AXI_AWVALID(S_AXI_AWVALID),
        .S_AXI_AWREADY(S_AXI_AWREADY),
        .S_AXI_WDATA(S_AXI_WDATA),
        .S_AXI_WSTRB(S_AXI_WSTRB),
        .S_AXI_WVALID(S_AXI_WVALID),
        .S_AXI_WREADY(S_AXI_WREADY),
        .S_AXI_BRESP(S_AXI_BRESP),
        .S_AXI_BVALID(S_AXI_BVALID),
        .S_AXI_BREADY(S_AXI_BREADY),
        .S_AXI_ARADDR(S_AXI_ARADDR),
        .S_AXI_ARVALID(S_AXI_ARVALID),
        .S_AXI_ARREADY(S_AXI_ARREADY),
        .S_AXI_RDATA(S_AXI_RDATA),
        .S_AXI_RRESP(S_AXI_RRESP),
        .S_AXI_RVALID(S_AXI_RVALID),
        .S_AXI_RREADY(S_AXI_RREADY),
        .pmod_ja(pmod_ja),
        .pmod_jb_i(pmod_jb_i)
    );

    reg        connected = 1'b0;
    reg        wr_ok;
    reg        rd_ok;
    reg [31:0] rd_val;

    // pmod_ja -> pin group 0 (the 8LD)
    always @(pmod_ja) if (connected) zs_pin_out(0, pmod_ja, $time);
    // pin group 1 (the Pmod BTN) -> pmod_jb_i, sampled every microsecond
    always #1000 pmod_jb_i <= zs_pin_in(1);

    task lite_write(input [31:0] a, input [31:0] d, input [3:0] st, output ok);
        begin
            @(posedge aclk);
            S_AXI_AWADDR <= a[4:0]; S_AXI_WDATA <= d; S_AXI_WSTRB <= st;
            S_AXI_AWVALID <= 1'b1; S_AXI_WVALID <= 1'b1;
            @(posedge aclk);
            while (!(S_AXI_AWREADY && S_AXI_WREADY)) @(posedge aclk);
            S_AXI_AWVALID <= 1'b0; S_AXI_WVALID <= 1'b0; S_AXI_BREADY <= 1'b1;
            @(posedge aclk);
            while (!S_AXI_BVALID) @(posedge aclk);
            ok = (S_AXI_BRESP == 2'b00);
            S_AXI_BREADY <= 1'b0;
        end
    endtask

    task lite_read(input [31:0] a, output [31:0] d, output ok);
        begin
            @(posedge aclk);
            S_AXI_ARADDR <= a[4:0]; S_AXI_ARVALID <= 1'b1;
            @(posedge aclk);
            while (!S_AXI_ARREADY) @(posedge aclk);
            S_AXI_ARVALID <= 1'b0; S_AXI_RREADY <= 1'b1;
            @(posedge aclk);
            while (!S_AXI_RVALID) @(posedge aclk);
            d = S_AXI_RDATA; ok = (S_AXI_RRESP == 2'b00);
            S_AXI_RREADY <= 1'b0;
        end
    endtask


    integer a_i, wd_i, ws_i, iw_i, st;
    longint tgt, rts;

    initial begin
        repeat (20) @(posedge aclk);
        aresetn = 1'b1;
        repeat (5) @(posedge aclk);
        rp_lockstep(1);
        if (rp_open("") != 0) begin
            $display("cosim_top: rp_open failed (is RP_SOCKET set and QEMU running?)");
            $finish;
        end
        $display("cosim_top: remote-port connected");
        connected = 1'b1;

        forever begin
            st = rp_next($time, -1, a_i, wd_i, ws_i, iw_i);
            if (st == 0) begin
                $display("cosim_top: peer closed, finishing at %0t", $time);
                rp_close();
                $finish;
            end else if (st == 3) begin
                tgt = rp_sync_target();
                if (tgt > $time) #(tgt - $time);
                rp_sync_ack($time);
            end else if (st == 2) begin
                repeat (20) @(posedge aclk);
            end else begin
                rts = rp_req_ts();
                if (rts > $time) #(rts - $time);
                if (iw_i != 0 && a_i[7:0] == 8'h3C && wd_i == 32'hC051D0DE) begin
                    $display("cosim_top: end-of-test marker at %0t", $time);
                    rp_done_t(0, 1, $time);
                    rp_close();
                    $finish;
                end else if (iw_i != 0) begin
                    lite_write(a_i, wd_i, ws_i[3:0], wr_ok);
                    rp_done_t(0, wr_ok, $time);
                end else begin
                    lite_read(a_i, rd_val, rd_ok);
                    rp_done_t(rd_val, rd_ok, $time);
                end
            end
        end
    end
endmodule
