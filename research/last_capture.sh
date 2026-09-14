#!/bin/zsh
# 查看最近一次抓包会话的双向字节流
d=$(ls -td /Users/yiyi/github/tdxrs/research/captures/*/ | head -1)
echo "== 会话: $d"
cat "$d/meta.txt" 2>/dev/null
echo "-- 客户端->主站 ($(wc -c < "$d/c2s.bin" | tr -d ' ') 字节):"
xxd "$d/c2s.bin" | head -24
echo "-- 主站->客户端 ($(wc -c < "$d/s2c.bin" | tr -d ' ') 字节, 前192字节):"
xxd "$d/s2c.bin" | head -12
