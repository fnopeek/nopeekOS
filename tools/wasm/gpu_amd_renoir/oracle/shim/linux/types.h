#pragma once
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
typedef uint8_t u8; typedef uint16_t u16; typedef uint32_t u32; typedef uint64_t u64;
typedef int8_t s8; typedef int16_t s16; typedef int32_t s32; typedef int64_t s64;
typedef uint16_t __le16; typedef uint32_t __le32; typedef uint64_t __le64;
#define le16_to_cpu(x) (x)
#define le32_to_cpu(x) (x)
#define cpu_to_le16(x) (x)
#define cpu_to_le32(x) (x)
#define __packed __attribute__((packed))
#define __counted_by(x)
#define le64_to_cpu(x) (x)
#define DECLARE_FLEX_ARRAY(TYPE, NAME) struct { struct { } __empty_##NAME; TYPE NAME[]; }
