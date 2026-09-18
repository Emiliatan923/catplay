#ifndef __IAP2_SCAN_H
#define __IAP2_SCAN_H

#include <linux/completion.h>
#include <linux/if.h>
#include <linux/kref.h>
#include <linux/types.h>

struct usb_device;
struct usb_interface;
struct device;

struct iap2_acc_accessory {
    struct kref kref;
    struct usb_device *udev;
    u16 vendor_id;
    u16 product_id;
    u8 busnum;
    u8 devnum;
    u8 iap2_ifnum;
    char manufacturer[64];
    char product[64];
    char ifname[IFNAMSIZ];
    /*
     * udev renames the NCM netdev shortly after probe (eth0 -> enx<mac>), so the
     * name snapshot above goes stale. ifindex survives renames and the live name
     * is looked up from it on read. Do NOT hold a net_device reference here:
     * keeping one past the accessory lifetime makes unregister_netdevice() spin
     * forever waiting for the refcount to drop.
     */
    int ifindex;
    char iap2_devnode[64];
    struct completion disconnected;
    atomic_t gone;
};

struct iap2_acc_accessory *iap2_acc_probe_accessory(void);
void iap2_acc_put_accessory(struct iap2_acc_accessory *acc);
bool iap2_acc_is_gone(struct iap2_acc_accessory *acc);
int iap2_char_devnode_path(struct usb_interface *intf, char *buf, size_t size);
struct device *iap2_acc_device_get(struct iap2_acc_accessory *acc);

#endif
