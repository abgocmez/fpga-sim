`timescale 1ns / 1ps
// rgb_pwm: AXI4-Lite PWM for the two RGB LEDs (LD5, LD6) of the Zybo Z7.
//   0x00 LD5_R  0x04 LD5_G  0x08 LD5_B  0x0C LD6_R  0x10 LD6_G  0x14 LD6_B
//   duty 0..255 each (RW). PWM period = 256 steps of 16 clocks.
// rgb_leds[5:0] = { LD6_B, LD6_G, LD6_R, LD5_B, LD5_G, LD5_R }
module rgb_pwm (
    input  wire        S_AXI_ACLK,
    input  wire        S_AXI_ARESETN,
    input  wire [4:0]  S_AXI_AWADDR,
    input  wire        S_AXI_AWVALID,
    output wire        S_AXI_AWREADY,
    input  wire [31:0] S_AXI_WDATA,
    input  wire [3:0]  S_AXI_WSTRB,
    input  wire        S_AXI_WVALID,
    output wire        S_AXI_WREADY,
    output wire [1:0]  S_AXI_BRESP,
    output wire        S_AXI_BVALID,
    input  wire        S_AXI_BREADY,
    input  wire [4:0]  S_AXI_ARADDR,
    input  wire        S_AXI_ARVALID,
    output wire        S_AXI_ARREADY,
    output wire [31:0] S_AXI_RDATA,
    output wire [1:0]  S_AXI_RRESP,
    output wire        S_AXI_RVALID,
    input  wire        S_AXI_RREADY,
    output wire [5:0]  rgb_leds
);
    reg        bvalid_r, rvalid_r;
    reg [31:0] rdata_r;
    reg [7:0]  duty [0:5];
    reg [3:0]  pre;
    reg [7:0]  ramp;
    integer    i;

    wire wr = S_AXI_AWVALID & S_AXI_WVALID & ~bvalid_r;
    wire rd = S_AXI_ARVALID & ~rvalid_r;
    assign S_AXI_AWREADY = wr;
    assign S_AXI_WREADY  = wr;
    assign S_AXI_BRESP   = 2'b00;
    assign S_AXI_BVALID  = bvalid_r;
    assign S_AXI_ARREADY = rd;
    assign S_AXI_RDATA   = rdata_r;
    assign S_AXI_RRESP   = 2'b00;
    assign S_AXI_RVALID  = rvalid_r;

    genvar g;
    generate
        for (g = 0; g < 6; g = g + 1) begin : pwm
            assign rgb_leds[g] = (ramp < duty[g]);
        end
    endgenerate

    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            bvalid_r <= 0; rvalid_r <= 0; rdata_r <= 0; pre <= 0; ramp <= 0;
            for (i = 0; i < 6; i = i + 1) duty[i] <= 8'd0;
        end else begin
            pre <= pre + 4'd1;
            if (pre == 4'd15) ramp <= ramp + 8'd1;
            if (wr) begin
                bvalid_r <= 1'b1;
                if (S_AXI_AWADDR[4:2] < 3'd6) duty[S_AXI_AWADDR[4:2]] <= S_AXI_WDATA[7:0];
            end else if (bvalid_r && S_AXI_BREADY) begin
                bvalid_r <= 1'b0;
            end
            if (rd) begin
                rvalid_r <= 1'b1;
                rdata_r  <= (S_AXI_ARADDR[4:2] < 3'd6) ? {24'd0, duty[S_AXI_ARADDR[4:2]]} : 32'd0;
            end else if (rvalid_r && S_AXI_RREADY) begin
                rvalid_r <= 1'b0;
            end
        end
    end
endmodule
