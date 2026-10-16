-- my_math_ip in VHDL: the same AXI4-Lite slave as fixtures/rtl/math/my_math_ip.v
--   0x00 slv_reg0  RW  32-bit operand
--   0x04 slv_reg1  RO  slv_reg0 * 2 (truncated to 32 bits)
-- Written by hand to exercise the xvhdl path of the export reader and the XSim adapter
-- (mixed-language elaboration with the Verilog testbench fixtures/export/rtl/cosim_top.sv).
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity my_math_ip is
    generic (
        C_S_AXI_DATA_WIDTH : integer := 32;
        C_S_AXI_ADDR_WIDTH : integer := 4
    );
    port (
        S_AXI_ACLK    : in  std_logic;
        S_AXI_ARESETN : in  std_logic;
        S_AXI_AWADDR  : in  std_logic_vector(C_S_AXI_ADDR_WIDTH-1 downto 0);
        S_AXI_AWPROT  : in  std_logic_vector(2 downto 0);
        S_AXI_AWVALID : in  std_logic;
        S_AXI_AWREADY : out std_logic;
        S_AXI_WDATA   : in  std_logic_vector(C_S_AXI_DATA_WIDTH-1 downto 0);
        S_AXI_WSTRB   : in  std_logic_vector((C_S_AXI_DATA_WIDTH/8)-1 downto 0);
        S_AXI_WVALID  : in  std_logic;
        S_AXI_WREADY  : out std_logic;
        S_AXI_BRESP   : out std_logic_vector(1 downto 0);
        S_AXI_BVALID  : out std_logic;
        S_AXI_BREADY  : in  std_logic;
        S_AXI_ARADDR  : in  std_logic_vector(C_S_AXI_ADDR_WIDTH-1 downto 0);
        S_AXI_ARPROT  : in  std_logic_vector(2 downto 0);
        S_AXI_ARVALID : in  std_logic;
        S_AXI_ARREADY : out std_logic;
        S_AXI_RDATA   : out std_logic_vector(C_S_AXI_DATA_WIDTH-1 downto 0);
        S_AXI_RRESP   : out std_logic_vector(1 downto 0);
        S_AXI_RVALID  : out std_logic;
        S_AXI_RREADY  : in  std_logic
    );
end entity my_math_ip;

architecture rtl of my_math_ip is
    signal slv_reg0 : std_logic_vector(C_S_AXI_DATA_WIDTH-1 downto 0) := (others => '0');
    signal bvalid   : std_logic := '0';
    signal rvalid   : std_logic := '0';
    signal rdata    : std_logic_vector(C_S_AXI_DATA_WIDTH-1 downto 0) := (others => '0');
    signal write_en : std_logic;
    signal read_en  : std_logic;
    signal slv_reg1 : std_logic_vector(C_S_AXI_DATA_WIDTH-1 downto 0);
begin
    slv_reg1 <= slv_reg0(C_S_AXI_DATA_WIDTH-2 downto 0) & '0';

    write_en <= S_AXI_AWVALID and S_AXI_WVALID and not bvalid;
    read_en  <= S_AXI_ARVALID and not rvalid;

    S_AXI_AWREADY <= write_en;
    S_AXI_WREADY  <= write_en;
    S_AXI_BRESP   <= "00";
    S_AXI_BVALID  <= bvalid;
    S_AXI_ARREADY <= read_en;
    S_AXI_RDATA   <= rdata;
    S_AXI_RRESP   <= "00";
    S_AXI_RVALID  <= rvalid;

    write_p : process (S_AXI_ACLK)
    begin
        if rising_edge(S_AXI_ACLK) then
            if S_AXI_ARESETN = '0' then
                slv_reg0 <= (others => '0');
                bvalid   <= '0';
            else
                if write_en = '1' then
                    if S_AXI_AWADDR(C_S_AXI_ADDR_WIDTH-1 downto 2) = "00" then
                        for i in 0 to (C_S_AXI_DATA_WIDTH/8)-1 loop
                            if S_AXI_WSTRB(i) = '1' then
                                slv_reg0(i*8+7 downto i*8) <= S_AXI_WDATA(i*8+7 downto i*8);
                            end if;
                        end loop;
                    end if;
                    bvalid <= '1';
                elsif bvalid = '1' and S_AXI_BREADY = '1' then
                    bvalid <= '0';
                end if;
            end if;
        end if;
    end process write_p;

    read_p : process (S_AXI_ACLK)
    begin
        if rising_edge(S_AXI_ACLK) then
            if S_AXI_ARESETN = '0' then
                rvalid <= '0';
                rdata  <= (others => '0');
            else
                if read_en = '1' then
                    if S_AXI_ARADDR(C_S_AXI_ADDR_WIDTH-1 downto 2) = "00" then
                        rdata <= slv_reg0;
                    elsif S_AXI_ARADDR(C_S_AXI_ADDR_WIDTH-1 downto 2) = "01" then
                        rdata <= slv_reg1;
                    else
                        rdata <= (others => '0');
                    end if;
                    rvalid <= '1';
                elsif rvalid = '1' and S_AXI_RREADY = '1' then
                    rvalid <= '0';
                end if;
            end if;
        end if;
    end process read_p;
end architecture rtl;
