`timescale 1ns / 1ps
// cosim_pcam: testbench top for the Pcam 5C demo. Stands in for the PS7 ports
// (M_AXI_GP0 slave side, S_AXI_HP0 master side) and wires up
//
//   0x43C00000  i2c_master  SCCB master (fixtures/rtl/pmod/i2c_master.v); its pins
//               are the camera connector, pin group 5, answered by the host's
//               OV5640 model through zs_pin_xchg
//   0x43C10000  video_fb    frame capture of the camera's pixel bus into DDR
//               (AXI master on S_AXI_HP0)
//
// The camera's frames reach the PL through the video input of zs_media.sv
// (`zs_hdmi_rx_src`, the same path as HDMI RX): there is no MIPI CSI-2 or
// D-PHY here, the sensor model hands the PL a parallel pixel bus.
// The frame is 160x120 to keep a frame period short.
module cosim_pcam;
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
    import "DPI-C" function int  zs_pin_xchg(input int group, input int levels, input longint now_ns);

    localparam int HP0_DEV  = 2;
    localparam int DDR_LAT  = 16;
    localparam int CAM_GROUP = 5;

    reg aclk = 1'b0;
    reg aresetn = 1'b0;
    always #5 aclk = ~aclk;                  // 100 MHz

    reg connected = 1'b0;

    // ---------------- AXI4-Lite master (PS -> PL) ----------------
    reg  [5:0]  m_awaddr_l = 0, m_araddr_l = 0;
    reg         awvalid = 0, wvalid = 0, bready = 0, arvalid = 0, rready = 0;
    reg  [31:0] wdata_l = 0;
    reg  [3:0]  wstrb_l = 0;
    reg         sel_video = 0;

    wire        c_awready, c_wready, c_bvalid, c_arready, c_rvalid;
    wire [1:0]  c_bresp, c_rresp;
    wire [31:0] c_rdata;
    wire        v_awready, v_wready, v_bvalid, v_arready, v_rvalid;
    wire [1:0]  v_bresp, v_rresp;
    wire [31:0] v_rdata;

    wire s_awready = sel_video ? v_awready : c_awready;
    wire s_wready  = sel_video ? v_wready  : c_wready;
    wire s_bvalid  = sel_video ? v_bvalid  : c_bvalid;
    wire [1:0] s_bresp = sel_video ? v_bresp : c_bresp;
    wire s_arready = sel_video ? v_arready : c_arready;
    wire s_rvalid  = sel_video ? v_rvalid  : c_rvalid;
    wire [1:0] s_rresp = sel_video ? v_rresp : c_rresp;
    wire [31:0] s_rdata = sel_video ? v_rdata : c_rdata;

    // ---------------- camera connector pins (group 5) ----------------
    wire [7:0] cam_o;
    reg  [7:0] cam_i = '1;                   // driven by the host's OV5640 model

    i2c_master sccb (
        .S_AXI_ACLK(aclk), .S_AXI_ARESETN(aresetn),
        .S_AXI_AWADDR(m_awaddr_l[4:0]), .S_AXI_AWVALID(awvalid & ~sel_video),
        .S_AXI_AWREADY(c_awready),
        .S_AXI_WDATA(wdata_l), .S_AXI_WSTRB(wstrb_l), .S_AXI_WVALID(wvalid & ~sel_video),
        .S_AXI_WREADY(c_wready),
        .S_AXI_BRESP(c_bresp), .S_AXI_BVALID(c_bvalid), .S_AXI_BREADY(bready & ~sel_video),
        .S_AXI_ARADDR(m_araddr_l[4:0]), .S_AXI_ARVALID(arvalid & ~sel_video),
        .S_AXI_ARREADY(c_arready),
        .S_AXI_RDATA(c_rdata), .S_AXI_RRESP(c_rresp), .S_AXI_RVALID(c_rvalid),
        .S_AXI_RREADY(rready & ~sel_video),
        .pmod_ja_o(cam_o), .pmod_ja_i(cam_i)
    );

    // Every change of the pins is answered by the board's devices at once.
    always @(cam_o) if (connected) cam_i <= zs_pin_xchg(CAM_GROUP, cam_o, $time);

    // ---------------- video input (the sensor's pixel bus) ----------------
    wire        rx_pclk, rx_de, rx_hs, rx_vs;
    wire [23:0] rx_rgb;
    wire        pclk_o, hs_o, vs_o, de_o;
    wire [23:0] rgb_o;

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

    video_fb #(.H_ACT(160), .V_ACT(120)) video (
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

    zs_hdmi_rx_src #(.H_ACT(160), .V_ACT(120)) camera (
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
        rp_lockstep(1);
        if (rp_open("") != 0) begin
            $display("cosim_pcam: rp_open failed");
            $finish;
        end
        connected = 1'b1;
        cam_i <= zs_pin_xchg(CAM_GROUP, cam_o, $time);
        $display("cosim_pcam: remote-port connected");

        forever begin
            st = rp_next($time, -1, a_i, wd_i, ws_i, iw_i);
            if (st == 0) begin
                $display("cosim_pcam: peer closed, finishing at %0t", $time);
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
                sel_video = a_i[16];                      // 0x43C1xxxx -> video_fb
                if (iw_i != 0 && a_i[7:0] == 8'h3C && !a_i[16] && wd_i == 32'hC051D0DE) begin
                    $display("cosim_pcam: end-of-test marker at %0t", $time);
                    rp_done_t(0, 1, $time);
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
