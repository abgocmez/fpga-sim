`timescale 1ns / 1ps
// zs_fir: an 8-tap FIR filter with an AXI4-Lite slave and a level interrupt (ZynqSim showcase).
//   y[n] = sum_{k=0..7} COEF[k] * x[n-k]      (signed 16-bit x and COEF, signed 32-bit y)
//
//   off   name        access   meaning
//   0x00  CTRL        RW       bit0 ENABLE, bit1 IRQ_EN, bit2 CLEAR (write 1: clears the delay line,
//                              COUNT and DONE; reads 0)
//   0x04  STATUS      RO/W1C   bit0 BUSY, bit1 DONE (sticky; write 1 to bit1 clears it)
//   0x08  COUNT       RO       samples processed since CLEAR
//   0x0C  ID          RO       0x5A464952 ("ZFIR")
//   0x10..0x2C COEF0..COEF7 RW signed 16 bit in [15:0] (read back sign-extended), reset 0
//   0x40  DIN         WO       signed 16-bit sample; accepted only when ENABLE=1 and BUSY=0: it is
//                              shifted into the delay line and starts a computation
//   0x44  DOUT        RO       the latest y[n]
// A computation is 8 multiply-accumulate cycles and one finishing cycle, then DONE=1, BUSY=0.
// irq = DONE & CTRL.IRQ_EN (level). A DIN while BUSY=1 or ENABLE=0 is ignored. Every transaction
// completes with OKAY; a read of an unlisted offset returns 0, a write to one is ignored.
module zs_fir #(
    parameter integer C_S_AXI_DATA_WIDTH = 32,
    parameter integer C_S_AXI_ADDR_WIDTH = 7
) (
    (* X_INTERFACE_INFO = "xilinx.com:signal:clock:1.0 S_AXI_ACLK CLK" *)
    (* X_INTERFACE_PARAMETER = "ASSOCIATED_BUSIF S_AXI, ASSOCIATED_RESET S_AXI_ARESETN" *)
    input  wire                                  S_AXI_ACLK,
    (* X_INTERFACE_INFO = "xilinx.com:signal:reset:1.0 S_AXI_ARESETN RST" *)
    (* X_INTERFACE_PARAMETER = "POLARITY ACTIVE_LOW" *)
    input  wire                                  S_AXI_ARESETN,

    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWADDR" *)
    input  wire [C_S_AXI_ADDR_WIDTH-1:0]         S_AXI_AWADDR,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWPROT" *)
    input  wire [2:0]                            S_AXI_AWPROT,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWVALID" *)
    input  wire                                  S_AXI_AWVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWREADY" *)
    output wire                                  S_AXI_AWREADY,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *)
    input  wire [C_S_AXI_DATA_WIDTH-1:0]         S_AXI_WDATA,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WSTRB" *)
    input  wire [(C_S_AXI_DATA_WIDTH/8)-1:0]     S_AXI_WSTRB,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WVALID" *)
    input  wire                                  S_AXI_WVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WREADY" *)
    output wire                                  S_AXI_WREADY,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BRESP" *)
    output wire [1:0]                            S_AXI_BRESP,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BVALID" *)
    output wire                                  S_AXI_BVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BREADY" *)
    input  wire                                  S_AXI_BREADY,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARADDR" *)
    input  wire [C_S_AXI_ADDR_WIDTH-1:0]         S_AXI_ARADDR,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARPROT" *)
    input  wire [2:0]                            S_AXI_ARPROT,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARVALID" *)
    input  wire                                  S_AXI_ARVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARREADY" *)
    output wire                                  S_AXI_ARREADY,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *)
    output wire [C_S_AXI_DATA_WIDTH-1:0]         S_AXI_RDATA,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RRESP" *)
    output wire [1:0]                            S_AXI_RRESP,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RVALID" *)
    output wire                                  S_AXI_RVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RREADY" *)
    input  wire                                  S_AXI_RREADY,

    (* X_INTERFACE_INFO = "xilinx.com:signal:interrupt:1.0 irq INTERRUPT" *)
    (* X_INTERFACE_PARAMETER = "SENSITIVITY LEVEL_HIGH" *)
    output wire                                  irq
);

    // Registers.
    reg                 ctrl_en;
    reg                 ctrl_irq_en;
    reg                 busy;
    reg                 done;
    reg  [31:0]         count;
    reg  [15:0]         coef [0:7];
    reg  [15:0]         xd   [0:7];     // delay line: xd[0] is the newest sample
    reg  [31:0]         dout;
    reg  signed [31:0]  acc;            // signed: the products are sign-extended into it
    reg  [3:0]          idx;

    reg                 bvalid;
    reg                 rvalid;
    reg  [31:0]         rdata;

    // AW and W are accepted together, one write in flight at a time.
    wire write_en = S_AXI_AWVALID & S_AXI_WVALID & ~bvalid;
    wire read_en  = S_AXI_ARVALID & ~rvalid;
    wire [4:0] waddr = S_AXI_AWADDR[6:2];
    wire [4:0] raddr = S_AXI_ARADDR[6:2];
    wire [4:0] rmin  = raddr - 5'd4;

    assign S_AXI_AWREADY = write_en;
    assign S_AXI_WREADY  = write_en;
    assign S_AXI_BRESP   = 2'b00;
    assign S_AXI_BVALID  = bvalid;
    assign S_AXI_ARREADY = read_en;
    assign S_AXI_RDATA   = rdata;
    assign S_AXI_RRESP   = 2'b00;
    assign S_AXI_RVALID  = rvalid;
    assign irq           = done & ctrl_irq_en;

    // A coefficient word with the byte strobes applied to the old value.
    wire [4:0]  cmin  = waddr - 5'd4;
    wire [2:0]  cidx  = cmin[2:0];
    wire [15:0] cword = {S_AXI_WSTRB[1] ? S_AXI_WDATA[15:8] : coef[cidx][15:8],
                         S_AXI_WSTRB[0] ? S_AXI_WDATA[7:0]  : coef[cidx][7:0]};

    integer i;
    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            ctrl_en     <= 1'b0;
            ctrl_irq_en <= 1'b0;
            busy        <= 1'b0;
            done        <= 1'b0;
            count       <= 32'd0;
            dout        <= 32'd0;
            acc         <= 32'sd0;
            idx         <= 4'd0;
            bvalid      <= 1'b0;
            for (i = 0; i < 8; i = i + 1) begin
                coef[i] <= 16'd0;
                xd[i]   <= 16'd0;
            end
        end else begin
            // ---- computation: 8 MAC cycles, then one to publish the result ----
            if (busy) begin
                if (idx < 4'd8) begin
                    acc <= acc + $signed(coef[idx[2:0]]) * $signed(xd[idx[2:0]]);
                    idx <= idx + 4'd1;
                end else begin
                    dout <= acc;
                    busy <= 1'b0;
                    done <= 1'b1;
                end
            end

            // ---- bus write (later assignments win over the computation above) ----
            if (write_en) begin
                bvalid <= 1'b1;
                if (waddr == 5'd0) begin                       // CTRL
                    if (S_AXI_WSTRB[0]) begin
                        ctrl_en     <= S_AXI_WDATA[0];
                        ctrl_irq_en <= S_AXI_WDATA[1];
                        if (S_AXI_WDATA[2]) begin              // CLEAR
                            count <= 32'd0;
                            done  <= 1'b0;
                            busy  <= 1'b0;
                            for (i = 0; i < 8; i = i + 1)
                                xd[i] <= 16'd0;
                        end
                    end
                end else if (waddr == 5'd1) begin              // STATUS, W1C on DONE
                    if (S_AXI_WSTRB[0] && S_AXI_WDATA[1])
                        done <= 1'b0;
                end else if (waddr >= 5'd4 && waddr <= 5'd11) begin   // COEF0..COEF7
                    coef[cidx] <= cword;
                end else if (waddr == 5'd16) begin             // DIN
                    if (ctrl_en && !busy) begin
                        xd[0] <= S_AXI_WDATA[15:0];
                        for (i = 1; i < 8; i = i + 1)
                            xd[i] <= xd[i-1];
                        count <= count + 32'd1;
                        busy  <= 1'b1;
                        done  <= 1'b0;
                        acc   <= 32'sd0;
                        idx   <= 4'd0;
                    end
                end
            end else if (bvalid && S_AXI_BREADY) begin
                bvalid <= 1'b0;
            end
        end
    end

    // ---- bus read ----
    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            rvalid <= 1'b0;
            rdata  <= 32'd0;
        end else begin
            if (read_en) begin
                rvalid <= 1'b1;
                if (raddr == 5'd0)       rdata <= {29'd0, 1'b0, ctrl_irq_en, ctrl_en};
                else if (raddr == 5'd1)  rdata <= {30'd0, done, busy};
                else if (raddr == 5'd2)  rdata <= count;
                else if (raddr == 5'd3)  rdata <= 32'h5A464952;
                else if (raddr >= 5'd4 && raddr <= 5'd11)
                    rdata <= {{16{coef[rmin[2:0]][15]}}, coef[rmin[2:0]]};
                else if (raddr == 5'd17) rdata <= dout;
                else                     rdata <= 32'd0;
            end else if (rvalid && S_AXI_RREADY) begin
                rvalid <= 1'b0;
            end
        end
    end

endmodule
