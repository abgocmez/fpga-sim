`timescale 1ns / 1ps
// cosim_top for dma_xform. Stands in for the PS7 ports the design uses:
//   M_AXI_GP0  : QEMU -> AXI4-Lite master -> dma_xform control registers
//   S_AXI_HP0  : dma_xform AXI4 master -> DPI -> QEMU memory (DDR)
//   IRQ_F2P[0] : dma_xform irq -> DPI -> QEMU GIC (SPI 29 = IRQ 61)
module cosim_top;
    import "DPI-C" function int  rp_open(input string descr);
    import "DPI-C" function int  rp_next(input longint now_ns, input int idle_ms,
                                         output int addr, output int wdata,
                                         output int wstrb, output int is_write);
    import "DPI-C" function void rp_done(input int rdata, input int ok);
    import "DPI-C" function int  rp_poll(input int timeout_ms);
    import "DPI-C" function void rp_mem_put(input int idx, input int word);
    import "DPI-C" function int  rp_mem_start(input longint now_ns, input int dev,
                                              input int is_write, input int addr, input int len);
    import "DPI-C" function int  rp_mem_poll(input int slot);
    import "DPI-C" function int  rp_mem_wait(input int slot);
    import "DPI-C" function int  rp_mem_get(input int slot, input int idx);
    import "DPI-C" function void rp_mem_free(input int slot);
    import "DPI-C" function void rp_irq(input longint now_ns, input int line, input int val);
    import "DPI-C" function void rp_close();
    import "DPI-C" function void tel_publish(input string json);
    import "DPI-C" function void tel_event(input string json);
    import "DPI-C" function void rp_lockstep(input int on);
    import "DPI-C" function longint rp_sync_target();
    import "DPI-C" function void rp_sync_ack(input longint now_ns);
    import "DPI-C" function longint rp_req_ts();
    import "DPI-C" function void rp_done_t(input int rdata, input int ok, input longint now_ns);

    localparam int HP0_DEV  = 2;     // remote-port device of S_AXI_HP0 (afi_0)
    localparam int DDR_LAT  = 16;    // modelled DDR access latency seen by the PL master (clocks)
    localparam int LOCKSTEP = 1;     // 1: PL time is locked to QEMU virtual time (needs -icount)

    reg aclk = 1'b0;
    reg aresetn = 1'b0;
    always #5 aclk = ~aclk;          // 100 MHz

    reg connected = 1'b0;

    // ---- statistics for the UI ----
    longint c_ps_rd = 0, c_ps_wr = 0, c_dma_rd_b = 0, c_dma_wr_b = 0;
    longint c_dma_rd_w = 0, c_dma_wr_w = 0, c_irq = 0, c_busy_clk = 0;
    longint last_ps_lat = 0;

    // ---------------- AXI4-Lite master (PS -> PL) ----------------
    reg  [5:0]  s_awaddr = 0, s_araddr = 0;
    reg         s_awvalid = 0, s_wvalid = 0, s_bready = 0, s_arvalid = 0, s_rready = 0;
    reg  [31:0] s_wdata = 0;
    reg  [3:0]  s_wstrb = 0;
    wire        s_awready, s_wready, s_bvalid, s_arready, s_rvalid;
    wire [1:0]  s_bresp, s_rresp;
    wire [31:0] s_rdata;

    // ---------------- AXI4 master (PL -> DDR) ----------------
    wire [31:0] m_araddr, m_awaddr, m_wdata;
    wire [7:0]  m_arlen, m_awlen;
    wire [2:0]  m_arsize, m_awsize;
    wire [1:0]  m_arburst, m_awburst;
    wire        m_arvalid, m_awvalid, m_wvalid, m_wlast, m_rready, m_bready;
    wire [3:0]  m_wstrb;
    reg         m_arready = 0, m_awready = 0, m_wready = 0;
    reg         m_rvalid = 0, m_rlast = 0, m_bvalid = 0;
    reg  [31:0] m_rdata = 0;
    reg  [1:0]  m_bresp = 0, m_rresp = 0;
    wire        irq;

    dma_xform dut (
        .aclk(aclk), .aresetn(aresetn),
        .s_awaddr(s_awaddr), .s_awvalid(s_awvalid), .s_awready(s_awready),
        .s_wdata(s_wdata), .s_wstrb(s_wstrb), .s_wvalid(s_wvalid), .s_wready(s_wready),
        .s_bresp(s_bresp), .s_bvalid(s_bvalid), .s_bready(s_bready),
        .s_araddr(s_araddr), .s_arvalid(s_arvalid), .s_arready(s_arready),
        .s_rdata(s_rdata), .s_rresp(s_rresp), .s_rvalid(s_rvalid), .s_rready(s_rready),
        .m_araddr(m_araddr), .m_arlen(m_arlen), .m_arsize(m_arsize), .m_arburst(m_arburst),
        .m_arvalid(m_arvalid), .m_arready(m_arready),
        .m_rdata(m_rdata), .m_rresp(m_rresp), .m_rlast(m_rlast), .m_rvalid(m_rvalid), .m_rready(m_rready),
        .m_awaddr(m_awaddr), .m_awlen(m_awlen), .m_awsize(m_awsize), .m_awburst(m_awburst),
        .m_awvalid(m_awvalid), .m_awready(m_awready),
        .m_wdata(m_wdata), .m_wstrb(m_wstrb), .m_wlast(m_wlast), .m_wvalid(m_wvalid), .m_wready(m_wready),
        .m_bresp(m_bresp), .m_bvalid(m_bvalid), .m_bready(m_bready),
        .irq(irq)
    );

    task lite_write(input [5:0] a, input [31:0] d, input [3:0] s, output ok);
        begin
            @(posedge aclk);
            s_awaddr <= a; s_wdata <= d; s_wstrb <= s;
            s_awvalid <= 1'b1; s_wvalid <= 1'b1;
            @(posedge aclk);
            while (!(s_awready && s_wready)) @(posedge aclk);
            s_awvalid <= 1'b0; s_wvalid <= 1'b0; s_bready <= 1'b1;
            @(posedge aclk);
            while (!s_bvalid) @(posedge aclk);
            ok = (s_bresp == 2'b00);
            s_bready <= 1'b0;
        end
    endtask

    task lite_read(input [5:0] a, output [31:0] d, output ok);
        begin
            @(posedge aclk);
            s_araddr <= a; s_arvalid <= 1'b1;
            @(posedge aclk);
            while (!s_arready) @(posedge aclk);
            s_arvalid <= 1'b0; s_rready <= 1'b1;
            @(posedge aclk);
            while (!s_rvalid) @(posedge aclk);
            d = s_rdata; ok = (s_rresp == 2'b00);
            s_rready <= 1'b0;
        end
    endtask

    // ---------------- QEMU side service loop ----------------
    integer a_i, wd_i, ws_i, iw_i, st;
    reg [31:0] rd;
    reg        ok;
    longint    tgt, rts;

    initial begin
        repeat (20) @(posedge aclk);
        aresetn = 1'b1;
        repeat (5) @(posedge aclk);

        rp_lockstep(LOCKSTEP);
        if (rp_open("") != 0) begin
            $display("cosim_top: rp_open failed (is RP_SOCKET set and QEMU running?)");
            $finish;
        end
        connected = 1'b1;
        $display("cosim_top: remote-port connected");

        forever begin
            st = rp_next($time, LOCKSTEP ? -1 : 1, a_i, wd_i, ws_i, iw_i);
            if (st == 0) begin
                $display("cosim_top: peer closed, finishing at %0t", $time);
                rp_close();
                $finish;
            end else if (st == 3) begin
                // QEMU reached virtual time tgt: run the PL up to it, then release QEMU.
                tgt = rp_sync_target();
                if (tgt > $time) #(tgt - $time);
                rp_sync_ack($time);
            end else if (st == 2) begin
                repeat (20) @(posedge aclk);
            end else begin
                rts = rp_req_ts();                   // the access happened at QEMU time rts
                if (LOCKSTEP && rts > $time) #(rts - $time);
                if (iw_i != 0 && a_i[7:0] == 8'h3C && wd_i == 32'hC051D0DE) begin
                    // simulation control: the firmware signals "end of test"
                    $display("cosim_top: end-of-test marker at %0t", $time);
                    rp_done_t(0, 1, $time);
                    rp_close();
                    $finish;
                end else if (iw_i != 0) begin
                    lite_write(a_i[5:0], wd_i, ws_i[3:0], ok);
                    c_ps_wr++;
                    last_ps_lat = $time - rts;
                    tel_event($sformatf("{\"t\":%0d,\"k\":\"ps_wr\",\"a\":%0d,\"d\":%0d,\"lat\":%0d}", rts, a_i, wd_i, $time - rts));
                    $display("cosim_top: PS WR @%08h <= %08h ok=%0d t=%0t", a_i, wd_i, ok, $time);
                    rp_done_t(0, ok, $time);
                end else begin
                    lite_read(a_i[5:0], rd, ok);
                    c_ps_rd++;
                    last_ps_lat = $time - rts;
                    tel_event($sformatf("{\"t\":%0d,\"k\":\"ps_rd\",\"a\":%0d,\"d\":%0d,\"lat\":%0d}", rts, a_i, rd, $time - rts));
                    $display("cosim_top: PS RD @%08h => %08h ok=%0d t=%0t", a_i, rd, ok, $time);
                    rp_done_t(rd, ok, $time);
                end
            end
        end
    end

    // ---------------- AXI4 slave bridge: read channel ----------------
    reg [31:0] ra; reg [7:0] rl; integer rslot, rst, ri;
    initial begin
        wait (connected);
        forever begin
            @(posedge aclk);
            if (m_arvalid && !m_arready) begin
                ra = m_araddr; rl = m_arlen;
                m_arready <= 1'b1;
                @(posedge aclk);
                m_arready <= 1'b0;
                rslot = rp_mem_start($time, HP0_DEV, 0, ra, (rl + 1) * 4);
                rst = rp_mem_wait(rslot);     // real-time wait, zero simulated time
                repeat (DDR_LAT) @(posedge aclk);
                c_dma_rd_b++; c_dma_rd_w = c_dma_rd_w + rl + 1;
                tel_event($sformatf("{\"t\":%0d,\"k\":\"dma_rd\",\"a\":%0d,\"w\":%0d}", $time, ra, rl + 1));
                $display("cosim_top: DMA RD  @%08h len=%0d words status=%0d t=%0t", ra, rl + 1, rst, $time);
                for (ri = 0; ri <= rl; ri = ri + 1) begin
                    m_rdata  <= (rst == 1) ? rp_mem_get(rslot, ri) : 32'hBAD0_BAD0;
                    m_rresp  <= (rst == 1) ? 2'b00 : 2'b10;
                    m_rvalid <= 1'b1;
                    m_rlast  <= (ri == rl);
                    @(posedge aclk);
                    while (!m_rready) @(posedge aclk);
                    m_rvalid <= 1'b0; m_rlast <= 1'b0;
                end
                rp_mem_free(rslot);
            end
        end
    end

    // ---------------- AXI4 slave bridge: write channel ----------------
    reg [31:0] wa; reg [7:0] wl; integer wslot, wst, wi;
    initial begin
        wait (connected);
        forever begin
            @(posedge aclk);
            if (m_awvalid && !m_awready) begin
                wa = m_awaddr; wl = m_awlen;
                m_awready <= 1'b1;
                @(posedge aclk);
                m_awready <= 1'b0;
                for (wi = 0; wi <= wl; wi = wi + 1) begin
                    m_wready <= 1'b1;
                    @(posedge aclk);
                    while (!m_wvalid) @(posedge aclk);
                    rp_mem_put(wi, m_wdata);
                    m_wready <= 1'b0;
                end
                wslot = rp_mem_start($time, HP0_DEV, 1, wa, (wl + 1) * 4);
                wst = rp_mem_wait(wslot);
                repeat (DDR_LAT) @(posedge aclk);
                c_dma_wr_b++; c_dma_wr_w = c_dma_wr_w + wl + 1;
                tel_event($sformatf("{\"t\":%0d,\"k\":\"dma_wr\",\"a\":%0d,\"w\":%0d}", $time, wa, wl + 1));
                $display("cosim_top: DMA WR  @%08h len=%0d words status=%0d t=%0t", wa, wl + 1, wst, $time);
                rp_mem_free(wslot);
                m_bvalid <= 1'b1; m_bresp <= (wst == 1) ? 2'b00 : 2'b10;
                @(posedge aclk);
                while (!m_bready) @(posedge aclk);
                m_bvalid <= 1'b0;
            end
        end
    end

    // ---------------- IRQ_F2P[0] -> QEMU ----------------
    reg irq_q = 1'b0;
    initial begin
        wait (connected);
        rp_irq($time, 0, 0);
        forever begin
            @(posedge aclk);
            if (irq !== irq_q) begin
                irq_q <= irq;
                rp_irq($time, 0, irq ? 1 : 0);
                if (irq) c_irq++;
                tel_event($sformatf("{\"t\":%0d,\"k\":\"irq\",\"v\":%0d}", $time, irq));
                $display("cosim_top: IRQ_F2P[0] -> %0d at %0t", irq, $time);
            end
        end
    end

    // ---------------- telemetry (UI) ----------------
    always @(posedge aclk) if (dut.busy) c_busy_clk++;

    initial begin
        wait (connected);
        forever begin
            #500;                                   // every 500 ns of simulated time
            tel_publish($sformatf({
                "{\"t\":%0d,\"lock\":%0d,\"quantum\":1000,\"ddr_lat\":%0d,",
                "\"state\":%0d,\"busy\":%0d,\"done\":%0d,\"err\":%0d,\"irq\":%0d,\"irq_en\":%0d,",
                "\"src\":%0d,\"dst\":%0d,\"len\":%0d,\"scale\":%0d,\"off\":%0d,",
                "\"cur_src\":%0d,\"cur_dst\":%0d,\"rem\":%0d,\"burst\":%0d,\"idx\":%0d,",
                "\"checksum\":%0d,\"words\":%0d,",
                "\"ps_rd\":%0d,\"ps_wr\":%0d,\"dma_rd_b\":%0d,\"dma_wr_b\":%0d,",
                "\"dma_rd_w\":%0d,\"dma_wr_w\":%0d,\"irqs\":%0d,\"busy_clk\":%0d,\"ps_lat\":%0d}"},
                $time, LOCKSTEP, DDR_LAT,
                dut.state, dut.busy, dut.done, dut.err, irq, dut.irq_en,
                dut.src_r, dut.dst_r, dut.len_r, dut.scale_r, dut.offset_r,
                dut.cur_src, dut.cur_dst, dut.rem, dut.n, dut.idx,
                dut.checksum, dut.words_done,
                c_ps_rd, c_ps_wr, c_dma_rd_b, c_dma_wr_b,
                c_dma_rd_w, c_dma_wr_w, c_irq, c_busy_clk, last_ps_lat));
        end
    end
endmodule
