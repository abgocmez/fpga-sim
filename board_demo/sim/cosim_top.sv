`timescale 1ns / 1ps
// cosim_top for the "virtual Zybo" demo.
//   0x43C00000  dma_xform (image brightness/contrast engine, HP0 master, IRQ_F2P[0])
//   0x43C10000  gpio_ctrl (4 LEDs, 4 switches, 4 buttons, IRQ_F2P[1])
// Switches/buttons come from the UI (control.txt), LEDs go back to it (telemetry).
module cosim_top;
    import "DPI-C" function int  rp_open(input string descr);
    import "DPI-C" function int  rp_next(input longint now_ns, input int idle_ms,
                                         output int addr, output int wdata,
                                         output int wstrb, output int is_write);
    import "DPI-C" function int  rp_poll(input int timeout_ms);
    import "DPI-C" function void rp_mem_put(input int idx, input int word);
    import "DPI-C" function int  rp_mem_start(input longint now_ns, input int dev,
                                              input int is_write, input int addr, input int len);
    import "DPI-C" function int  rp_mem_wait(input int slot);
    import "DPI-C" function int  rp_mem_get(input int slot, input int idx);
    import "DPI-C" function void rp_mem_free(input int slot);
    import "DPI-C" function void rp_irq(input longint now_ns, input int line, input int val);
    import "DPI-C" function void rp_close();
    import "DPI-C" function void rp_lockstep(input int on);
    import "DPI-C" function longint rp_sync_target();
    import "DPI-C" function void rp_sync_ack(input longint now_ns);
    import "DPI-C" function longint rp_req_ts();
    import "DPI-C" function void rp_done_t(input int rdata, input int ok, input longint now_ns);
    import "DPI-C" function void tel_publish(input string json);
    import "DPI-C" function void tel_event(input string json);
    import "DPI-C" function int  tel_ctrl(output int sw, output int btn);

    localparam int HP0_DEV  = 2;
    localparam int DDR_LAT  = 16;
    localparam int LOCKSTEP = 1;

    reg aclk = 1'b0;
    reg aresetn = 1'b0;
    always #5 aclk = ~aclk;
    reg connected = 1'b0;

    // ---------------- AXI4-Lite master (PS -> PL) with a 2-way address decode ----------------
    reg  [5:0]  m_awaddr_l = 0, m_araddr_l = 0;
    reg         awvalid = 0, wvalid = 0, bready = 0, arvalid = 0, rready = 0;
    reg  [31:0] wdata_l = 0;
    reg  [3:0]  wstrb_l = 0;
    reg         sel_gpio = 0;                      // 1: access targets gpio_ctrl

    wire        d_awready, d_wready, d_bvalid, d_arready, d_rvalid;
    wire [1:0]  d_bresp, d_rresp;
    wire [31:0] d_rdata;
    wire        g_awready, g_wready, g_bvalid, g_arready, g_rvalid;
    wire [1:0]  g_bresp, g_rresp;
    wire [31:0] g_rdata;

    wire        s_awready = sel_gpio ? g_awready : d_awready;
    wire        s_wready  = sel_gpio ? g_wready  : d_wready;
    wire        s_bvalid  = sel_gpio ? g_bvalid  : d_bvalid;
    wire [1:0]  s_bresp   = sel_gpio ? g_bresp   : d_bresp;
    wire        s_arready = sel_gpio ? g_arready : d_arready;
    wire        s_rvalid  = sel_gpio ? g_rvalid  : d_rvalid;
    wire [1:0]  s_rresp   = sel_gpio ? g_rresp   : d_rresp;
    wire [31:0] s_rdata   = sel_gpio ? g_rdata   : d_rdata;

    // ---------------- DMA engine AXI4 master ----------------
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
    wire        irq_dma, irq_gpio;

    // ---------------- board pins ----------------
    reg  [3:0]  sw_pins = 0, btn_pins = 0;
    wire [3:0]  led_pins;

    dma_xform dma (
        .aclk(aclk), .aresetn(aresetn),
        .s_awaddr(m_awaddr_l), .s_awvalid(awvalid & ~sel_gpio), .s_awready(d_awready),
        .s_wdata(wdata_l), .s_wstrb(wstrb_l), .s_wvalid(wvalid & ~sel_gpio), .s_wready(d_wready),
        .s_bresp(d_bresp), .s_bvalid(d_bvalid), .s_bready(bready & ~sel_gpio),
        .s_araddr(m_araddr_l), .s_arvalid(arvalid & ~sel_gpio), .s_arready(d_arready),
        .s_rdata(d_rdata), .s_rresp(d_rresp), .s_rvalid(d_rvalid), .s_rready(rready & ~sel_gpio),
        .m_araddr(m_araddr), .m_arlen(m_arlen), .m_arsize(m_arsize), .m_arburst(m_arburst),
        .m_arvalid(m_arvalid), .m_arready(m_arready),
        .m_rdata(m_rdata), .m_rresp(m_rresp), .m_rlast(m_rlast), .m_rvalid(m_rvalid), .m_rready(m_rready),
        .m_awaddr(m_awaddr), .m_awlen(m_awlen), .m_awsize(m_awsize), .m_awburst(m_awburst),
        .m_awvalid(m_awvalid), .m_awready(m_awready),
        .m_wdata(m_wdata), .m_wstrb(m_wstrb), .m_wlast(m_wlast), .m_wvalid(m_wvalid), .m_wready(m_wready),
        .m_bresp(m_bresp), .m_bvalid(m_bvalid), .m_bready(m_bready),
        .irq(irq_dma)
    );

    gpio_ctrl gpio (
        .aclk(aclk), .aresetn(aresetn),
        .s_awaddr(m_awaddr_l), .s_awvalid(awvalid & sel_gpio), .s_awready(g_awready),
        .s_wdata(wdata_l), .s_wvalid(wvalid & sel_gpio), .s_wready(g_wready),
        .s_bresp(g_bresp), .s_bvalid(g_bvalid), .s_bready(bready & sel_gpio),
        .s_araddr(m_araddr_l), .s_arvalid(arvalid & sel_gpio), .s_arready(g_arready),
        .s_rdata(g_rdata), .s_rresp(g_rresp), .s_rvalid(g_rvalid), .s_rready(rready & sel_gpio),
        .sw(sw_pins), .btn(btn_pins), .led(led_pins), .irq(irq_gpio)
    );

    task lite_write(input [5:0] a, input [31:0] d, input [3:0] s, output ok);
        begin
            @(posedge aclk);
            m_awaddr_l <= a; wdata_l <= d; wstrb_l <= s;
            awvalid <= 1'b1; wvalid <= 1'b1;
            @(posedge aclk);
            while (!(s_awready && s_wready)) @(posedge aclk);
            awvalid <= 1'b0; wvalid <= 1'b0; bready <= 1'b1;
            @(posedge aclk);
            while (!s_bvalid) @(posedge aclk);
            ok = (s_bresp == 2'b00);
            bready <= 1'b0;
        end
    endtask

    task lite_read(input [5:0] a, output [31:0] d, output ok);
        begin
            @(posedge aclk);
            m_araddr_l <= a; arvalid <= 1'b1;
            @(posedge aclk);
            while (!s_arready) @(posedge aclk);
            arvalid <= 1'b0; rready <= 1'b1;
            @(posedge aclk);
            while (!s_rvalid) @(posedge aclk);
            d = s_rdata; ok = (s_rresp == 2'b00);
            rready <= 1'b0;
        end
    endtask

    // ---- statistics ----
    longint c_ps_rd = 0, c_ps_wr = 0, c_dma_rd_b = 0, c_dma_wr_b = 0;
    longint c_dma_rd_w = 0, c_dma_wr_w = 0, c_irq = 0, c_busy_clk = 0, last_ps_lat = 0;

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
            $display("cosim_top: rp_open failed");
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
                tgt = rp_sync_target();
                if (tgt > $time) #(tgt - $time);
                rp_sync_ack($time);
            end else if (st == 2) begin
                repeat (20) @(posedge aclk);
            end else begin
                rts = rp_req_ts();
                if (LOCKSTEP && rts > $time) #(rts - $time);
                sel_gpio = a_i[16];                       // 0x43C1xxxx -> gpio_ctrl
                if (iw_i != 0 && a_i[7:0] == 8'h3C && !a_i[16] && wd_i == 32'hC051D0DE) begin
                    $display("cosim_top: end-of-test marker at %0t", $time);
                    rp_done_t(0, 1, $time);
                    rp_close();
                    $finish;
                end else if (iw_i != 0) begin
                    lite_write(a_i[5:0], wd_i, ws_i[3:0], ok);
                    c_ps_wr++; last_ps_lat = $time - rts;
                    tel_event($sformatf("{\"t\":%0d,\"k\":\"ps_wr\",\"a\":%0d,\"d\":%0d,\"lat\":%0d}", rts, a_i, wd_i, $time - rts));
                    rp_done_t(0, ok, $time);
                end else begin
                    lite_read(a_i[5:0], rd, ok);
                    c_ps_rd++; last_ps_lat = $time - rts;
                    tel_event($sformatf("{\"t\":%0d,\"k\":\"ps_rd\",\"a\":%0d,\"d\":%0d,\"lat\":%0d}", rts, a_i, rd, $time - rts));
                    rp_done_t(rd, ok, $time);
                end
            end
        end
    end

    // ---------------- AXI4 slave bridge: read channel (HP0) ----------------
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
                rst = rp_mem_wait(rslot);
                repeat (DDR_LAT) @(posedge aclk);
                c_dma_rd_b++; c_dma_rd_w = c_dma_rd_w + rl + 1;
                tel_event($sformatf("{\"t\":%0d,\"k\":\"dma_rd\",\"a\":%0d,\"w\":%0d}", $time, ra, rl + 1));
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

    // ---------------- AXI4 slave bridge: write channel (HP0) ----------------
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
                rp_mem_free(wslot);
                m_bvalid <= 1'b1; m_bresp <= (wst == 1) ? 2'b00 : 2'b10;
                @(posedge aclk);
                while (!m_bready) @(posedge aclk);
                m_bvalid <= 1'b0;
            end
        end
    end

    // ---------------- IRQ_F2P[1:0] -> QEMU ----------------
    reg [1:0] irq_q = 2'b00;
    wire [1:0] irq_now = {irq_gpio, irq_dma};
    integer il;
    initial begin
        wait (connected);
        rp_irq($time, 0, 0);
        rp_irq($time, 1, 0);
        forever begin
            @(posedge aclk);
            for (il = 0; il < 2; il = il + 1)
                if (irq_now[il] !== irq_q[il]) begin
                    irq_q[il] <= irq_now[il];
                    if (irq_now[il]) c_irq++;
                    rp_irq($time, il, irq_now[il] ? 1 : 0);
                    tel_event($sformatf("{\"t\":%0d,\"k\":\"irq\",\"line\":%0d,\"v\":%0d}", $time, il, irq_now[il]));
                end
        end
    end

    // ---------------- board inputs (UI) and LED events ----------------
    integer ctl_sw, ctl_btn, ctl_r;
    reg [3:0] led_q = 4'h0;
    initial begin
        wait (connected);
        forever begin
            #200;
            ctl_r = tel_ctrl(ctl_sw, ctl_btn);
            if (sw_pins !== ctl_sw[3:0] || btn_pins !== ctl_btn[3:0]) begin
                tel_event($sformatf("{\"t\":%0d,\"k\":\"board\",\"sw\":%0d,\"btn\":%0d}", $time, ctl_sw[3:0], ctl_btn[3:0]));
                sw_pins  <= ctl_sw[3:0];
                btn_pins <= ctl_btn[3:0];
            end
            if (led_pins !== led_q) begin
                led_q <= led_pins;
                tel_event($sformatf("{\"t\":%0d,\"k\":\"led\",\"v\":%0d}", $time, led_pins));
            end
        end
    end

    // ---------------- telemetry (UI) ----------------
    always @(posedge aclk) if (dma.busy) c_busy_clk++;

    initial begin
        wait (connected);
        forever begin
            #500;
            tel_publish($sformatf({
                "{\"t\":%0d,\"lock\":%0d,\"quantum\":1000,\"ddr_lat\":%0d,",
                "\"state\":%0d,\"busy\":%0d,\"done\":%0d,\"err\":%0d,\"irq\":%0d,\"irq_en\":%0d,",
                "\"src\":%0d,\"dst\":%0d,\"len\":%0d,\"scale\":%0d,\"off\":%0d,",
                "\"cur_src\":%0d,\"cur_dst\":%0d,\"rem\":%0d,\"burst\":%0d,\"idx\":%0d,",
                "\"checksum\":%0d,\"words\":%0d,",
                "\"ps_rd\":%0d,\"ps_wr\":%0d,\"dma_rd_b\":%0d,\"dma_wr_b\":%0d,",
                "\"dma_rd_w\":%0d,\"dma_wr_w\":%0d,\"irqs\":%0d,\"busy_clk\":%0d,\"ps_lat\":%0d,",
                "\"led\":%0d,\"sw\":%0d,\"btn\":%0d,\"gpio_irq\":%0d}"},
                $time, LOCKSTEP, DDR_LAT,
                dma.state, dma.busy, dma.done, dma.err, irq_dma, dma.irq_en,
                dma.src_r, dma.dst_r, dma.len_r, dma.scale_r, dma.offset_r,
                dma.cur_src, dma.cur_dst, dma.rem, dma.n, dma.idx,
                dma.checksum, dma.words_done,
                c_ps_rd, c_ps_wr, c_dma_rd_b, c_dma_wr_b,
                c_dma_rd_w, c_dma_wr_w, c_irq, c_busy_clk, last_ps_lat,
                led_pins, sw_pins, btn_pins, irq_gpio));
        end
    end
endmodule
