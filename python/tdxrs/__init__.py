"""tdxrs - 通达信行情数据解析库 (Rust 实现)

核心模块:
- Reader: 日线、分钟线、板块、财务数据解析
- Client: 行情客户端 (TdxHqClient, AsyncTdxHqClient, TdxDirectClient, TdxSmartClient, TdxHqFundClient, TdxBlockClient)

用法:
    from tdxrs import TdxHqClient, TdxSmartClient, DailyBarReader
"""

try:
    from tdxrs._internal import (
        DailyBarReader, MinBarReader, LcMinBarReader, BlockReader, FinancialReader,
        TdxHqClient, AsyncTdxHqClient, TdxDirectClient, TdxSmartClient, TdxHqFundClient, TdxBlockClient,
    )
except ImportError:
    raise ImportError(
        "tdxrs native module not found. Please install with: pip install tdxrs"
    )

__version__ = "0.6.8"
__all__ = [
    "DailyBarReader", "MinBarReader", "LcMinBarReader", "BlockReader", "FinancialReader",
    "TdxHqClient", "AsyncTdxHqClient", "TdxDirectClient", "TdxSmartClient", "TdxHqFundClient", "TdxBlockClient",
]
