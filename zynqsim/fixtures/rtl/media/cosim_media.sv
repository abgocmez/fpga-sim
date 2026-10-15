`timescale 1ns / 1ps
// cosim_media: testbench top for the audio + HDMI demo. Stands in for the PS7
// ports (M_AXI_GP0 slave side, S_AXI_HP0 master side) and carries the board's
// codec and HDMI connector models (zs_media.sv).
//
//   0x43C00000  audio_ctl   (I2S master + bit-banged I2C to the SSM2603)
//   0x43C10000  video_fb    (frame-buffer scan-out and capture, AXI master -> DDR)
module cosim_media;
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
    import "DPI-C" function void rp_mem_put(input int idx, input int word);
    import "DPI-C" function int  rp_mem_start(input longint now_ns, input int dev,
                                              input int is_write, input int addr, input int len);
    import "DPI-C" function int  rp_mem_wait(input int slot);
    import "DPI-C" function int  rp_mem_get(input int slot, input int idx);
    import "DPI-C" function void rp_mem_free(input int slot);
    import "DPI-C" function void zs_audio_flush();

    localparam int HP0_DEV  = 2;
    localparam int DDR_LAT  = 16;
    localparam int LOCKSTEP = 1;

    reg aclk = 1'b0;
    reg aresetn = 1'b0;
    always #5 aclk = ~aclk;                  // 100 MHz

    reg mclk = 1'b0;                         // 12.288 MHz audio master clock
    always #40.690 mclk = ~mclk;

    reg connected = 1'b0;

    // ---------------- AXI4-Lite master (PS -> PL) ----------------
    reg  [5:0]  m_awaddr_l = 0, m_araddr_l = 0;
    reg         awvalid = 0, wvalid = 0, bready = 0, arvalid = 0, rready = 0;
    reg  [31:0] wdata_l = 0;
    reg  [3:0]  wstrb_l = 0;
    reg         sel_video = 0;

    wire        a_awready, a_wready, a_bvalid, a_arready, a_rvalid;
    wire [1:0]  a_bresp, a_rresp;
    wire [31:0] a_rdata;
    wire        v_awready, v_wready, v_bvalid, v_arready, v_rvalid;
    wire [1:0]  v_bresp, v_rresp;
    wire [31:0] v_rdata;

    wire s_awready = sel_video ? v_awready : a_awready;
    wire s_wready  = sel_video ? v_wready  : a_wready;
    wire s_bvalid  = sel_video ? v_bvalid  : a_bvalid;
    wire [1:0] s_bresp = sel_video ? v_bresp : a_bresp;
    wire s_arready = sel_video ? v_arready : a_arready;
    wire s_rvalid  = sel_video ? v_rvalid  : a_rvalid;
    wire [1:0] s_rresp = sel_video ? v_rresp : a_rresp;
    wire [31:0] s_rdata = sel_video ? v_rdata : a_rdata;

    // ---------------- audio pins ----------------
    wire bclk, lrclk, pbdat, recdat, scl_o, sda_o, sda_i;
    wire sda_line, scl_line;
    pullup (sda_line);
    pullup (scl_line);
    assign sda_line = sda_o ? 1'bz : 1'b0;   // open-drain master
    assign scl_line = scl_o ? 1'bz : 1'b0;
    assign sda_i = sda_line;

    audio_ctl audio (
        .aclk(aclk), .aresetn(aresetn),
        .s_awaddr(m_awaddr_l), .s_awvalid(awvalid & ~sel_video), .s_awready(a_awready),
        .s_wdata(wdata_l), .s_wvalid(wvalid & ~sel_video), .s_wready(a_wready),
        .s_bresp(a_bresp), .s_bvalid(a_bvalid), .s_bready(bready & ~sel_video),
        .s_araddr(m_araddr_l), .s_arvalid(arvalid & ~sel_video), .s_arready(a_arready),
        .s_rdata(a_rdata), .s_rresp(a_rresp), .s_rvalid(a_rvalid), .s_rready(rready & ~sel_video),
        .mclk(mclk), .bclk(bclk), .lrclk(lrclk), .pbdat(pbdat), .recdat(recdat),
        .scl_o(scl_o), .sda_o(sda_o), .sda_i(sda_i)
    );

    zs_ssm2603 codec (
        .bclk(bclk), .lrclk(lrclk), .dac_data(pbdat), .adc_data(recdat),
        .scl(scl_line), .sda(sda_line)
    );

    // ---------------- video pins ----------------
    wire        pclk_o, hs_o, vs_o, de_o;
    wire [23:0] rgb_o;
    wire        rx_pclk, rx_de, rx_hs, rx_vs;
    wire [23:0] rx_rgb;

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

    video_fb video (
        .aclk(aclk), .aresetn(aresetn),
        .s_awaddr(m_awaddr_l), .s_awvalid(awvalid & sel_video), .s_awready(v_awready),
        .s_wdata(wdata_l), .s_wvalid(wvalid & sel_video), .s_wready(v_wready),
        .s_bresp(v_bresp), .s_bvalid(v_bvalid), .s_bready(bready & sel_video),
        .s_araddr(m_araddr_l), .s_arvalid(arvalid & sel_video), .s_arready(v_arready),
        .s_rdata(v_rdata), .s_rresp(v_rresp), .s_rvalid(v_rvalid), .s_rready(rready & sel_video),
        .m_araddr(m_araddr), .m_arlen(m_arlen), .m_arsize(m_arsize), .m_arburst(m_arburst),
        .m_arvalid(m_arvalid), .m_arready(m_arready),
        .m_rdata(m_rdata), .m_rresp(m_rresp), .m_rlast(m_rlast), .m_rvalid(m_rvalid), .m_rready(m_rready),
        .m_awaddr(m_awaddr), .m_awlen(m_awlen), .m_awsize(m_awsize), .m_awburst(m_awburst),
        .m_awvalid(m_awvalid), .m_awready(m_awready),
        .m_wdata(m_wdata), .m_wstrb(m_wstrb), .m_wlast(m_wlast), .m_wvalid(m_wvalid), .m_wready(m_wready),
        .m_bresp(m_bresp), .m_bvalid(m_bvalid), .m_bready(m_bready),
        .pclk_o(pclk_o), .hs_o(hs_o), .vs_o(vs_o), .de_o(de_o), .rgb_o(rgb_o),
        .pclk_i(rx_pclk), .hs_i(rx_hs), .vs_i(rx_vs), .de_i(rx_de), .rgb_i(rx_rgb)
    );

    zs_hdmi_tx_sink #(.VS_ACTIVE_LOW(1)) hdmi_tx (
        .pclk(pclk_o), .de(de_o), .hs(hs_o), .vs(vs_o), .rgb(rgb_o)
    );

    zs_hdmi_rx_src hdmi_rx (
        .ref_clk(aclk), .enable(connected),
        .pclk(rx_pclk), .de(rx_de), .hs(rx_hs), .vs(rx_vs), .rgb(rx_rgb)
    );

    // ---------------- AXI4-Lite master tasks ----------------
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

    // ---------------- PS side service loop ----------------
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
            $display("cosim_media: rp_open failed");
            $finish;
        end
        connected = 1'b1;
        $display("cosim_media: remote-port connected");

        forever begin
            st = rp_next($time, LOCKSTEP ? -1 : 1, a_i, wd_i, ws_i, iw_i);
            if (st == 0) begin
                $display("cosim_media: peer closed, finishing at %0t", $time);
                zs_audio_flush();
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
                sel_video = a_i[16];                      // 0x43C1xxxx -> video_fb
                if (iw_i != 0 && a_i[7:0] == 8'h3C && !a_i[16] && wd_i == 32'hC051D0DE) begin
                    $display("cosim_media: end-of-test marker at %0t", $time);
                    rp_done_t(0, 1, $time);
                    zs_audio_flush();
                    rp_close();
                    $finish;
                end else if (iw_i != 0) begin
                    lite_write(a_i[5:0], wd_i, ws_i[3:0], ok);
                    rp_done_t(0, ok, $time);
                end else begin
                    lite_read(a_i[5:0], rd, ok);
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
                rp_mem_free(wslot);
                m_bvalid <= 1'b1; m_bresp <= (wst == 1) ? 2'b00 : 2'b10;
                @(posedge aclk);
                while (!m_bready) @(posedge aclk);
                m_bvalid <= 1'b0;
            end
        end
    end
endmodule
