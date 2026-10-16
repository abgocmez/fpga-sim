`timescale 1ns / 1ps
// ILLEGAL on purpose (ADR-025 test fixture): an AXI4-Lite slave (a copy of my_math_ip) and
// an AXI4 master. Each write to reg0 starts one write burst of four beats at 0x100, but
// WLAST is asserted on beat 1 instead of beat 3 (and so missing on beat 3). The protocol
// checker must report `AXI4 master W LAST_EARLY detail=0x1` and
// `AXI4 master W LAST_MISSING detail=0x3`.
module illegal_master_bad_wlast #(
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
    output wire                                  m_arvalid,
    input  wire                                  m_arready,
    input  wire [31:0]                           m_rdata,
    input  wire [1:0]                            m_rresp,
    input  wire                                  m_rlast,
    input  wire                                  m_rvalid,
    output wire                                  m_rready,
    output reg  [31:0]                           m_awaddr,
    output reg  [7:0]                            m_awlen,
    output wire [2:0]                            m_awsize,
    output wire [1:0]                            m_awburst,
    output reg                                   m_awvalid,
    input  wire                                  m_awready,
    output reg  [31:0]                           m_wdata,
    output wire [3:0]                            m_wstrb,
    output reg                                   m_wlast,
    output reg                                   m_wvalid,
    input  wire                                  m_wready,
    input  wire [1:0]                            m_bresp,
    input  wire                                  m_bvalid,
    output wire                                  m_bready
);
    reg  [C_S_AXI_DATA_WIDTH-1:0] slv_reg0;
    reg                           bvalid;
    reg                           rvalid;
    reg  [C_S_AXI_DATA_WIDTH-1:0] rdata;
    reg  [1:0]                    wbeat;      // beat index of the write burst

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

    assign m_araddr  = 32'd0;
    assign m_arlen   = 8'd0;
    assign m_arsize  = 3'd2;
    assign m_arburst = 2'b01;
    assign m_arvalid = 1'b0;
    assign m_rready  = 1'b1;
    assign m_awsize  = 3'd2;
    assign m_awburst = 2'b01;
    assign m_wstrb   = 4'hF;
    assign m_bready  = 1'b1;

    integer i;
    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            slv_reg0 <= {C_S_AXI_DATA_WIDTH{1'b0}};
            bvalid   <= 1'b0;
            m_awvalid <= 1'b0;
            m_wvalid  <= 1'b0;
            m_wlast   <= 1'b0;
            m_awlen   <= 8'd0;
            m_awaddr  <= 32'd0;
            m_wdata   <= 32'd0;
            wbeat     <= 2'd0;
        end else begin
            // The address phase: one burst of four beats per write to reg0.
            if (m_awvalid && m_awready) begin
                m_awvalid <= 1'b0;
                m_wvalid  <= 1'b1;
                wbeat     <= 2'd0;
                m_wdata   <= 32'hA5A5_0000;
                m_wlast   <= 1'b0;
            end
            // The data phase: WLAST on beat 1 (the defect), not on beat 3.
            if (m_wvalid && m_wready) begin
                wbeat   <= wbeat + 2'd1;
                m_wdata <= m_wdata + 32'd1;
                if (wbeat == 2'd3) begin
                    m_wvalid <= 1'b0;
                    m_wlast  <= 1'b0;
                end else begin
                    m_wlast <= (wbeat + 2'd1 == 2'd1);
                end
            end
            if (write_en) begin
                if (S_AXI_AWADDR[C_S_AXI_ADDR_WIDTH-1:2] == 2'd0) begin
                    for (i = 0; i < C_S_AXI_DATA_WIDTH/8; i = i + 1)
                        if (S_AXI_WSTRB[i])
                            slv_reg0[i*8 +: 8] <= S_AXI_WDATA[i*8 +: 8];
                    m_awaddr  <= 32'h0000_0100;
                    m_awlen   <= 8'd3;
                    m_awvalid <= 1'b1;
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
