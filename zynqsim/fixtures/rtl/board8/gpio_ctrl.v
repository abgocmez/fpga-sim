`timescale 1ns / 1ps
// gpio_ctrl (8-bit): the board I/O block of fixtures/rtl/board widened to the ZedBoard's
// 8 slide switches, 7 push buttons and 8 LEDs, behind AXI4-Lite. Same register map:
//   0x00 LED      RW  bits[7:0] drive led[7:0]
//   0x04 SW       RO  bits[7:0] slide switches
//   0x08 BTN      RO  bits[7:0] push buttons (raw, synchronised; bit 7 is unused)
//   0x0C BTN_EVT  W1C bit n set on a rising edge of button n
//   0x10 IRQ_EN   RW  bits[7:0] enable per-button interrupt
//   0x14 TICKS    RO  free-running clock counter (32-bit)
// irq = |(BTN_EVT & IRQ_EN)  (level, cleared by writing BTN_EVT)
module gpio_ctrl #(
    parameter integer W = 8
) (
    input  wire        aclk,
    input  wire        aresetn,
    input  wire [5:0]  s_awaddr,
    input  wire        s_awvalid,
    output wire        s_awready,
    input  wire [31:0] s_wdata,
    input  wire        s_wvalid,
    output wire        s_wready,
    output wire [1:0]  s_bresp,
    output wire        s_bvalid,
    input  wire        s_bready,
    input  wire [5:0]  s_araddr,
    input  wire        s_arvalid,
    output wire        s_arready,
    output wire [31:0] s_rdata,
    output wire [1:0]  s_rresp,
    output wire        s_rvalid,
    input  wire        s_rready,

    input  wire [W-1:0] sw,
    input  wire [W-1:0] btn,
    output wire [W-1:0] led,
    output wire        irq
);
    reg [W-1:0] led_r, irq_en, btn_s1, btn_s2, btn_d, btn_evt;
    reg [31:0]  ticks;
    reg         bvalid_r, rvalid_r;
    reg [31:0]  rdata_r;

    wire wr_en = s_awvalid & s_wvalid & ~bvalid_r;
    wire rd_en = s_arvalid & ~rvalid_r;

    assign s_awready = wr_en;
    assign s_wready  = wr_en;
    assign s_bresp   = 2'b00;
    assign s_bvalid  = bvalid_r;
    assign s_arready = rd_en;
    assign s_rdata   = rdata_r;
    assign s_rresp   = 2'b00;
    assign s_rvalid  = rvalid_r;
    assign led       = led_r;
    assign irq       = |(btn_evt & irq_en);

    wire [W-1:0] rise = btn_s2 & ~btn_d;   // btn_s2 is the synchronised value

    always @(posedge aclk) begin
        if (!aresetn) begin
            led_r <= 0; irq_en <= 0; btn_s1 <= 0; btn_s2 <= 0; btn_d <= 0;
            btn_evt <= 0; ticks <= 0; bvalid_r <= 0;
        end else begin
            ticks  <= ticks + 1;
            btn_s1 <= btn;
            btn_s2 <= btn_s1;
            btn_d  <= btn_s2;
            btn_evt <= btn_evt | rise;
            if (wr_en) begin
                bvalid_r <= 1'b1;
                case (s_awaddr[5:2])
                    4'd0: led_r  <= s_wdata[W-1:0];
                    4'd3: btn_evt <= (btn_evt | rise) & ~s_wdata[W-1:0];
                    4'd4: irq_en <= s_wdata[W-1:0];
                    default: ;
                endcase
            end else if (bvalid_r && s_bready) begin
                bvalid_r <= 1'b0;
            end
        end
    end

    always @(posedge aclk) begin
        if (!aresetn) begin
            rvalid_r <= 0; rdata_r <= 0;
        end else if (rd_en) begin
            rvalid_r <= 1'b1;
            case (s_araddr[5:2])
                4'd0:    rdata_r <= {{(32-W){1'b0}}, led_r};
                4'd1:    rdata_r <= {{(32-W){1'b0}}, sw};
                4'd2:    rdata_r <= {{(32-W){1'b0}}, btn_s2};
                4'd3:    rdata_r <= {{(32-W){1'b0}}, btn_evt};
                4'd4:    rdata_r <= {{(32-W){1'b0}}, irq_en};
                4'd5:    rdata_r <= ticks;
                default: rdata_r <= 0;
            endcase
        end else if (rvalid_r && s_rready) begin
            rvalid_r <= 1'b0;
        end
    end
endmodule
