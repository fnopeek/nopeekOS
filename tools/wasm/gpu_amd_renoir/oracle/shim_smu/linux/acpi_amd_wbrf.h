/* What amdgpu_smu.c's wbrf code names; Renoir has no wbrf. */
#define WBRF_CHANGED 1
#define NOTIFY_OK 1
#define NOTIFY_DONE 0
#define acpi_amd_wbrf_supported_consumer(dev) (abort(), 0)
