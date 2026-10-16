`timescale 1ns / 1ps
// cosim_top for the ZedBoard demo: the "virtual Zybo" testbench of fixtures/rtl/board with
// the board pins widened to 8 bits and the DMA left out.
//   0x43C00000  gpio_ctrl (8 LEDs, 8 switches, 8 buttons, IRQ_F2P[0])
// A write of 0xC051D0DE to offset 0x3C ends the run (the prototype's end-of-test marker).
//
// The testbench interface to the board (ADR-022): `tel_ctrl(sw, btn)` returns the switches
// and buttons as one integer each, bit n = component n of the board file (the ZedBoard has
// 8 switches and 7 buttons); `tel_event({"k":"led","v":N})` reports the LEDs the same way.
// The Zybo testbenches use the low four bits of the same words.
module cosim_top;
    import "DPI-C" function int  rp_open(input string descr);
    import "DPI-C" function int  rp_next(input longint now_ns, input int idle_ms,
                                         output int addr, output int wdata,
                                         output int wstrb, output int is_write);
    import "DPI-C" function int  rp_poll(input int timeout_ms);
    import "DPI-C" function void rp_irq(input longint now_ns, input int line, input int val);
    import "DPI-C" function void rp_close();
    import "DPI-C" function void rp_lockstep(input int on);
    import "DPI-C" function longint rp_sync_target();
    import "DPI-C" function void rp_sync_ack(input longint now_ns);
    import "DPI-C" function longint rp_req_ts();
    import "DPI-C" function void rp_done_t(input int rdata, input int ok, input longint now_ns);
    import "DPI-C" function void tel_event(input string json);
    import "DPI-C" function int  tel_ctrl(output int sw, output int btn);

    localparam int LOCKSTEP = 1;

    reg aclk = 1'b0;
    reg aresetn = 1'b0;
    always #5 aclk = ~aclk;
    reg connected = 1'b0;

    // ---------------- AXI4-Lite master (PS -> PL) ----------------
    reg  [5:0]  m_awaddr_l = 0, m_araddr_l = 0;
    reg         awvalid = 0, wvalid = 0, bready = 0, arvalid = 0, rready = 0;
    reg  [31:0] wdata_l = 0;
    reg  [3:0]  wstrb_l = 0;

    wire        g_awready, g_wready, g_bvalid, g_arready, g_rvalid;
    wire [1:0]  g_bresp, g_rresp;
    wire [31:0] g_rdata;
    wire        irq_gpio;

    // ---------------- board pins: 8 bits each ----------------
    reg  [7:0]  sw_pins = 0, btn_pins = 0;
    wire [7:0]  led_pins;

    gpio_ctrl #(.W(8)) gpio (
        .aclk(aclk), .aresetn(aresetn),
        .s_awaddr(m_awaddr_l), .s_awvalid(awvalid), .s_awready(g_awready),
        .s_wdata(wdata_l), .s_wvalid(wvalid), .s_wready(g_wready),
        .s_bresp(g_bresp), .s_bvalid(g_bvalid), .s_bready(bready),
        .s_araddr(m_araddr_l), .s_arvalid(arvalid), .s_arready(g_arready),
        .s_rdata(g_rdata), .s_rresp(g_rresp), .s_rvalid(g_rvalid), .s_rready(rready),
        .sw(sw_pins), .btn(btn_pins), .led(led_pins), .irq(irq_gpio)
    );

    task lite_write(input [5:0] a, input [31:0] d, input [3:0] s, output ok);
        begin
            @(posedge aclk);
            m_awaddr_l <= a; wdata_l <= d; wstrb_l <= s;
            awvalid <= 1'b1; wvalid <= 1'b1;
            @(posedge aclk);
            while (!(g_awready && g_wready)) @(posedge aclk);
            awvalid <= 1'b0; wvalid <= 1'b0; bready <= 1'b1;
            @(posedge aclk);
            while (!g_bvalid) @(posedge aclk);
            ok = (g_bresp == 2'b00);
            bready <= 1'b0;
        end
    endtask

    task lite_read(input [5:0] a, output [31:0] d, output ok);
        begin
            @(posedge aclk);
            m_araddr_l <= a; arvalid <= 1'b1;
            @(posedge aclk);
            while (!g_arready) @(posedge aclk);
            arvalid <= 1'b0; rready <= 1'b1;
            @(posedge aclk);
            while (!g_rvalid) @(posedge aclk);
            d = g_rdata; ok = (g_rresp == 2'b00);
            rready <= 1'b0;
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
                if (iw_i != 0 && a_i[7:0] == 8'h3C && wd_i == 32'hC051D0DE) begin
                    $display("cosim_top: end-of-test marker at %0t", $time);
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

    // ---------------- IRQ_F2P[0] -> QEMU ----------------
    reg irq_q = 1'b0;
    initial begin
        wait (connected);
        rp_irq($time, 0, 0);
        forever begin
            @(posedge aclk);
            if (irq_gpio !== irq_q) begin
                irq_q <= irq_gpio;
                rp_irq($time, 0, irq_gpio ? 1 : 0);
            end
        end
    end

    // ---------------- board inputs (UI) and LED events ----------------
    integer ctl_sw, ctl_btn, ctl_r;
    reg [7:0] led_q = 8'h00;
    initial begin
        wait (connected);
        forever begin
            #200;
            ctl_r = tel_ctrl(ctl_sw, ctl_btn);
            if (sw_pins !== ctl_sw[7:0] || btn_pins !== ctl_btn[7:0]) begin
                sw_pins  <= ctl_sw[7:0];
                btn_pins <= ctl_btn[7:0];
            end
            if (led_pins !== led_q) begin
                led_q <= led_pins;
                tel_event($sformatf("{\"t\":%0d,\"k\":\"led\",\"v\":%0d}", $time, led_pins));
            end
        end
    end
endmodule
