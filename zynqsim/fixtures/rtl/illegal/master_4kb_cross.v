`timescale 1ns / 1ps
// ILLEGAL on purpose (ADR-025 test fixture): an AXI4-Lite slave (a copy of my_math_ip) and
// an AXI4 master. Each write to reg0 starts one INCR read of four 32-bit beats at 0xFF8,
// which crosses the 4 KB boundary at 0x1000. The protocol checker must report
// `AXI4 master AR CROSS_4KB detail=0xff8` at the address handshake.
module illegal_master_4kb_cross #(
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
    input  wire                                  S_AXI_RREADY,
    // AXI4 master (driven by this module)
    output wire [31:0]                           m_araddr,
    output wire [7:0]                            m_arlen,
    output wire [2:0]                            m_arsize,
    output wire [1:0]                            m_arburst,
    output reg                                   m_arvalid,
    input  wire                                  m_arready,
    input  wire [31:0]                           m_rdata,
    input  wire [1:0]                            m_rresp,
    input  wire                                  m_rlast,
    input  wire                                  m_rvalid,
    output wire                                  m_rready,
    output wire [31:0]                           m_awaddr,
    output wire [7:0]                            m_awlen,
    output wire [2:0]                            m_awsize,
    output wire [1:0]                            m_awburst,
    output wire                                  m_awvalid,
    input  wire                                  m_awready,
    output wire [31:0]                           m_wdata,
    output wire [3:0]                            m_wstrb,
    output wire                                  m_wlast,
    output wire                                  m_wvalid,
    input  wire                                  m_wready,
    input  wire [1:0]                            m_bresp,
    input  wire                                  m_bvalid,
    output wire                                  m_bready
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
    assign S_AXI_RVALID  = rvalid;

    // The burst: four beats of 4 bytes from 0xFF8, across the 0x1000 boundary.
    assign m_araddr  = 32'h0000_0FF8;
    assign m_arlen   = 8'd3;
    assign m_arsize  = 3'd2;
    assign m_arburst = 2'b01;
    assign m_rready  = 1'b1;
    assign m_awaddr  = 32'd0;
    assign m_awlen   = 8'd0;
    assign m_awsize  = 3'd2;
    assign m_awburst = 2'b01;
    assign m_awvalid = 1'b0;
    assign m_wdata   = 32'd0;
    assign m_wstrb   = 4'd0;
    assign m_wlast   = 1'b0;
    assign m_wvalid  = 1'b0;
    assign m_bready  = 1'b1;

    integer i;
    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            slv_reg0  <= {C_S_AXI_DATA_WIDTH{1'b0}};
            bvalid    <= 1'b0;
            m_arvalid <= 1'b0;
        end else begin
            if (m_arvalid && m_arready) m_arvalid <= 1'b0;
            if (write_en) begin
                if (S_AXI_AWADDR[C_S_AXI_ADDR_WIDTH-1:2] == 2'd0) begin
                    for (i = 0; i < C_S_AXI_DATA_WIDTH/8; i = i + 1)
                        if (S_AXI_WSTRB[i])
                            slv_reg0[i*8 +: 8] <= S_AXI_WDATA[i*8 +: 8];
                    m_arvalid <= 1'b1;
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
