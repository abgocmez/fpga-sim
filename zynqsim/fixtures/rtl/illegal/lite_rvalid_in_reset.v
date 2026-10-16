`timescale 1ns / 1ps
// ILLEGAL on purpose (ADR-025 test fixture): a copy of my_math_ip whose RVALID is high
// while ARESETN is low. The protocol checker must report `AXI4-Lite slave R RESET_VALID`
// at 15 ns (the second reset cycle: the first edge at 5 ns is the reset's own).
module illegal_lite_rvalid_in_reset #(
    parameter integer C_S_AXI_DATA_WIDTH = 32,
    parameter integer C_S_AXI_ADDR_WIDTH = 4
) (
    input  wire                                  S_AXI_ACLK,
    input  wire                                  S_AXI_ARESETN,
    input  wire [C_S_AXI_ADDR_WIDTH-1:0]         S_AXI_AWADDR,
    input  wire [2:0]                            S_AXI_AWPROT,
    input  wire                                  S_AXI_AWVALID,
    output wire                                  S_AXI_AWREADY,
    input  wire [C_S_AXI_DATA_WIDTH-1:0]         S_AXI_WDATA,
    input  wire [(C_S_AXI_DATA_WIDTH/8)-1:0]     S_AXI_WSTRB,
    input  wire                                  S_AXI_WVALID,
    output wire                                  S_AXI_WREADY,
    output wire [1:0]                            S_AXI_BRESP,
    output wire                                  S_AXI_BVALID,
    input  wire                                  S_AXI_BREADY,
    input  wire [C_S_AXI_ADDR_WIDTH-1:0]         S_AXI_ARADDR,
    input  wire [2:0]                            S_AXI_ARPROT,
    input  wire                                  S_AXI_ARVALID,
    output wire                                  S_AXI_ARREADY,
    output wire [C_S_AXI_DATA_WIDTH-1:0]         S_AXI_RDATA,
    output wire [1:0]                            S_AXI_RRESP,
    output wire                                  S_AXI_RVALID,
    input  wire                                  S_AXI_RREADY
);
    reg  [C_S_AXI_DATA_WIDTH-1:0] slv_reg0;
    reg                           bvalid;
    reg                           rvalid;
    reg  [C_S_AXI_DATA_WIDTH-1:0] rdata;

    wire write_en = S_AXI_AWVALID & S_AXI_WVALID & ~bvalid;
    wire read_en  = S_AXI_ARVALID & ~rvalid;

    assign S_AXI_AWREADY = write_en;
    assign S_AXI_WREADY  = write_en;
    assign S_AXI_BRESP   = 2'b00;
    assign S_AXI_BVALID  = bvalid;
    assign S_AXI_ARREADY = read_en;
    assign S_AXI_RDATA   = rdata;
    assign S_AXI_RRESP   = 2'b00;
    // The defect: RVALID is driven high during reset.
    assign S_AXI_RVALID  = rvalid | ~S_AXI_ARESETN;

    integer i;
    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            slv_reg0 <= {C_S_AXI_DATA_WIDTH{1'b0}};
            bvalid   <= 1'b0;
        end else begin
            if (write_en) begin
                if (S_AXI_AWADDR[C_S_AXI_ADDR_WIDTH-1:2] == 2'd0) begin
                    for (i = 0; i < C_S_AXI_DATA_WIDTH/8; i = i + 1)
                        if (S_AXI_WSTRB[i])
                            slv_reg0[i*8 +: 8] <= S_AXI_WDATA[i*8 +: 8];
                end
                bvalid <= 1'b1;
            end else if (bvalid && S_AXI_BREADY) begin
                bvalid <= 1'b0;
            end
        end
    end

    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            rvalid <= 1'b0;
            rdata  <= {C_S_AXI_DATA_WIDTH{1'b0}};
        end else begin
            if (read_en) begin
                rdata  <= (S_AXI_ARADDR[C_S_AXI_ADDR_WIDTH-1:2] == 2'd0) ? slv_reg0 : 32'd0;
                rvalid <= 1'b1;
            end else if (rvalid && S_AXI_RREADY) begin
                rvalid <= 1'b0;
            end
        end
    end
endmodule
