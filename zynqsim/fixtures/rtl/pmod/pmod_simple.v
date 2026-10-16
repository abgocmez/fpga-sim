`timescale 1ns / 1ps
// pmod_simple: an AXI4-Lite register block for two simple Pmod modules.
//
//   0x00 LEDS  RW  drives pmod_ja[7:0]: a Digilent Pmod 8LD on JA (active high)
//   0x04 PINS  RO  pmod_jb_i[7:0] as read at the pins: a Pmod BTN on JB
//                  (bits 0..3 BTN0..3, bits 4..7 SW0..3; the board sets them)
//
// Nothing else. The testbench (pmod_simple_top.sv) reports pmod_ja to the host
// and samples pmod_jb_i from the host's levels once a microsecond.
module pmod_simple (
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
    output wire [7:0]  pmod_ja,
    input  wire [7:0]  pmod_jb_i
);
    reg        bvalid_r, rvalid_r;
    reg [31:0] rdata_r;
    reg [7:0]  leds;

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

    assign pmod_ja = leds;

    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            bvalid_r <= 0; rvalid_r <= 0; rdata_r <= 0; leds <= 8'd0;
        end else begin
            if (wr) begin
                bvalid_r <= 1'b1;
                if (S_AXI_AWADDR[4:2] == 3'd0) leds <= S_AXI_WDATA[7:0];
            end else if (bvalid_r && S_AXI_BREADY) begin
                bvalid_r <= 1'b0;
            end
            if (rd) begin
                rvalid_r <= 1'b1;
                case (S_AXI_ARADDR[4:2])
                    3'd0:    rdata_r <= {24'd0, leds};
                    3'd1:    rdata_r <= {24'd0, pmod_jb_i};
                    default: rdata_r <= 32'd0;
                endcase
            end else if (rvalid_r && S_AXI_RREADY) begin
                rvalid_r <= 1'b0;
            end
        end
    end
endmodule
