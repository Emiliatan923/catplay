// SPDX-License-Identifier: GPL-2.0
#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/container_of.h>
#include <linux/of.h>
#include <linux/of_platform.h>
#include <linux/platform_device.h>
#include <linux/property.h>
#include <linux/usb.h>
#include <linux/fs.h>
#include <linux/sysfs.h>
#include <linux/delay.h>
#include <linux/slab.h>

#include "iphone_dev.h"
#include "iap2_scan.h"

#define IPHONE_ROLE_SWITCH_REBIND_DEBOUNCE_MS 1000
#define IPHONE_ROLE_SWITCH_HOST_DELAY_MS 70

static int iphone_dev_set_otg_role(struct g_iphone *iphone_gadget,
					       enum usb_role role);
static int iphone_dev_set_otg_role_internal(struct iphone_dev_data *data,
					    enum usb_role role,
					    bool manage_gadget_lifecycle);
static void iphone_dev_status_notify_workfn(struct work_struct *work);
static void iphone_dev_role_switch_rebind_workfn(struct work_struct *work);
static bool iphone_dev_has_role_switch(struct iphone_dev_data *data);
static const char *iphone_dev_role_switch_name(const char *gadget_name);
static int iphone_dev_update_role_switch_name_from_gadget(struct iphone_dev_data *data);

static void iphone_dev_notify_status_changed(struct g_iphone *iphone_gadget)
{
	struct iphone_dev_data *data =
		container_of(iphone_gadget, struct iphone_dev_data, g);

	schedule_work(&data->status_notify_work);
}

static void iphone_dev_status_notify_workfn(struct work_struct *work)
{
	struct iphone_dev_data *data =
		container_of(work, struct iphone_dev_data, status_notify_work);
	struct device *owner_dev = READ_ONCE(data->owner_dev);

	if (!owner_dev)
		return;

	/*
	 * Pair with g_iphone_set_status() store-release so poll wakeups happen
	 * after the new value is globally visible to sysfs readers.
	 */
	sysfs_notify(&owner_dev->kobj, NULL, "status");
}

static void iphone_dev_remove_accessory_link_locked(struct iphone_dev_data *data)
{
	if (!data->owner_dev || !data->accessory_link_added)
		return;

	sysfs_remove_link(&data->owner_dev->kobj, "iap2_accessory");
	data->accessory_link_added = false;
}

static int iphone_dev_add_accessory_link_locked(struct iphone_dev_data *data,
						struct iap2_acc_accessory *acc)
{
	struct device *acc_dev;
	int ret;

	if (!data->owner_dev || !acc)
		return -ENODEV;

	acc_dev = iap2_acc_device_get(acc);
	if (!acc_dev)
		return -ENODEV;

	iphone_dev_remove_accessory_link_locked(data);
	ret = sysfs_create_link(&data->owner_dev->kobj, &acc_dev->kobj,
				"iap2_accessory");
	put_device(acc_dev);
	if (ret)
		return ret;

	data->accessory_link_added = true;
	return 0;
}

static int iphone_dev_unbind_gadget(struct iphone_dev_data *data)
{
	if (!data->driver_registered)
		return 0;

	pr_info("iPhone: unregistering gadget\n");
	usb_composite_unregister(&data->driver->drv);
	data->driver_registered = false;
	if (data->gadget_registered) {
		pr_warn("iPhone: gadget still marked registered after unregister\n");
		return -EBUSY;
	}

	/* After unbind, always leave USB_ROLE_DEVICE */
	int ret = iphone_dev_set_otg_role_internal(data, USB_ROLE_DEVICE, false);
	return ret;
}

static int iphone_dev_bind_gadget(struct iphone_dev_data *data)
{
	int ret;

	if (data->driver_registered)
		return 0;

	/* Before a rebind, always enter USB_ROLE_DEVICE. The first bind relies on
	 * the DT role-switch-default-mode because cdev does not exist yet. */
	if (iphone_dev_has_role_switch(data)) {
		ret = iphone_dev_set_otg_role_internal(data, USB_ROLE_DEVICE, false);
		if (ret)
			return ret;
	}

	pr_info("iPhone: registering gadget\n");
	ret = c2a_usb_composite_probe(&data->driver->drv);
	if (ret) {
		pr_warn("iPhone: gadget register failed: %d\n", ret);
		return ret;
	}
	data->driver_registered = true;

	return 0;
}

static void iphone_dev_role_switch_rebind_workfn(struct work_struct *work)
{
	struct iphone_dev_data *data =
		container_of(to_delayed_work(work), struct iphone_dev_data,
			     role_switch_rebind_work);
	int ret;

	data->role_switch_rebind_scheduled = false;

	if (g_iphone_get_status(&data->g) != RoleSwitchFailed)
		return;

	pr_info("iPhone: rebinding gadget after role-switch failure debounce\n");
	ret = iphone_dev_bind_gadget(data);
	if (ret) {
		pr_warn("iPhone: debounced gadget rebind failed: %d\n", ret);
		return;
	}

	g_iphone_set_status(&data->g, Initial);
}

static void iphone_dev_clear_accessory_locked(struct iphone_dev_data *data)
{
	struct iap2_acc_accessory *acc = data->acc;

	iphone_dev_remove_accessory_link_locked(data);
	data->acc = NULL;
	if (acc)
		iap2_acc_put_accessory(acc);
}

static void iphone_dev_accessory_watch_work(struct work_struct *work)
{
	struct iphone_dev_data *data =
		container_of(to_delayed_work(work), struct iphone_dev_data,
			     accessory_watch_work);
	struct iap2_acc_accessory *acc;
	bool disconnected = false;

	mutex_lock(&data->lock);
	acc = data->acc;
	if (iap2_acc_is_gone(acc)) {
		disconnected = true;
		iphone_dev_clear_accessory_locked(data);
	}
	mutex_unlock(&data->lock);

	if (!disconnected) {
		schedule_delayed_work(&data->accessory_watch_work,
				      msecs_to_jiffies(100));
		return;
	}

	pr_info("iPhone: accessory disconnected, reverting OTG role\n");
	iphone_dev_set_otg_role(&data->g, USB_ROLE_DEVICE);
	data->g.role_switch_requested = false;
	g_iphone_set_status(&data->g, Initial);
}

static int iphone_dev_set_otg_role(struct g_iphone *iphone_gadget,
				       enum usb_role role)
{
	struct iphone_dev_data *data =
		container_of(iphone_gadget, struct iphone_dev_data, g);
	return iphone_dev_set_otg_role_internal(data, role, true);
}

/*
 * Role switching is required by the CarPlay dongle protocol: the gadget first
 * enumerates as an Apple device (05ac:12a8) and, once the head unit sends the
 * 0x51 vendor request, the dongle must flip to USB_HOST and enumerate the
 * head unit's iAP2 + NCM interfaces.
 *
 * Two mechanisms are supported, in order of preference:
 *   1. the fwnode-backed usb_role_switch handle (dwc2/dwc3 with a DT
 *      role-switch description), and
 *   2. the usb_role class in sysfs, addressed by role-switch name, which is
 *      how some vendor kernels (e.g. V821/MUSB) expose the same control.
 *
 * There is deliberately no userspace "recovery" helper anymore: the old
 * carlinkit_otalib fallback hid real role-switch failures.
 */
static const char *iphone_dev_role_switch_name(const char *gadget_name)
{
	if (!gadget_name || !gadget_name[0])
		return NULL;

	if (!strcmp(gadget_name, "2184200.usb"))
		return "ci_hdrc.1";
	/* V821 exposes MUSB through a child UDC of the USB glue device. */
	if (!strcmp(gadget_name, "musb-hdrc.1.auto"))
		return "44100000.usb";

	return gadget_name;
}

static int iphone_dev_update_role_switch_name_from_gadget(struct iphone_dev_data *data)
{
	const char *gadget_name;
	const char *rs_name;

	if (!data)
		return -ENODEV;

	if (data->udc[0])
		gadget_name = data->udc;
	else if (data->cdev && data->cdev->gadget)
		gadget_name = data->cdev->gadget->name;
	else
		return -ENODEV;

	rs_name = iphone_dev_role_switch_name(gadget_name);
	if (!rs_name)
		return -ENODEV;

	strscpy(data->role_switch_name, rs_name, sizeof(data->role_switch_name));
	return 0;
}

/*
 * Path of the Rockchip USB2 PHY's otg_mode attribute for this controller.
 *
 * This is the control that actually works on rk3528: the PHY forces the iddig
 * line through the GRF, and the SoC's OTG state machine - which is what decides
 * the role once the DT says dr_mode = "otg" - reacts to that. Measured on a
 * ROCK 2A: writing only the controller's dwc3 debugfs "mode" switches it to
 * host and the state machine pulls it back within a second, while writing this
 * PHY attribute first holds. The PHY is the extcon provider named by the
 * controller's "extcon" property, so follow that phandle to its sysfs dir.
 */
static bool iphone_dev_rockchip_phy_mode_path(struct iphone_dev_data *data, char *path, size_t size)
{
	struct device *controller = NULL;
	struct device_node *phy_np;
	struct platform_device *phy_pdev;
	bool own_ref = false;
	int len;
	bool ok = false;

	/*
	 * Resolved once and then reused: the role switch unbinds the gadget
	 * before flipping, and cdev->gadget is gone by then.
	 */
	if (data->rockchip_phy_mode_path[0]) {
		strscpy(path, data->rockchip_phy_mode_path, size);
		return true;
	}

	if (data->cdev && data->cdev->gadget) {
		controller = data->cdev->gadget->dev.parent;
		if (!controller)
			controller = &data->cdev->gadget->dev;
	}

	/*
	 * The first bind can fail, and the role switch unbinds the gadget before
	 * flipping, so cdev is not a reliable way to reach the controller. Its
	 * platform device is named after the UDC, which is always known.
	 */
	if ((!controller || !controller->of_node) && data->udc[0]) {
		controller = bus_find_device_by_name(&platform_bus_type, NULL, data->udc);
		own_ref = controller != NULL;
	}

	if (!controller || !controller->of_node)
		goto out;

	phy_np = of_parse_phandle(controller->of_node, "extcon", 0);
	if (!phy_np)
		goto out;

	phy_pdev = of_find_device_by_node(phy_np);
	of_node_put(phy_np);
	if (!phy_pdev)
		goto out;

	len = snprintf(path, size, "/sys/devices/platform/%s/otg_mode", dev_name(&phy_pdev->dev));
	if (len > 0 && (size_t)len < size) {
		strscpy(data->rockchip_phy_mode_path, path, sizeof(data->rockchip_phy_mode_path));
		ok = true;
	}

	put_device(&phy_pdev->dev);

out:
	if (own_ref && controller)
		put_device(controller);
	return ok;
}

static bool path_is_openable(const char *path)
{
	struct file *file = filp_open(path, O_RDONLY, 0);

	if (IS_ERR(file))
		return false;
	filp_close(file, NULL);
	return true;
}

/*
 * True when a role-switch mechanism is actually usable right now.
 *
 * Every mechanism is probed for real - a configured UDC name alone proves
 * nothing, and treating it as a working switch is what made the first bind fail
 * with -ENODEV earlier. When none is usable the controller keeps its DT default
 * role instead of failing the bind.
 */
static bool iphone_dev_has_role_switch(struct iphone_dev_data *data)
{
	char path[256];
	int len;

	if (data->role_switch)
		return true;

	if (data->role_switch_name[0]) {
		len = snprintf(path, sizeof(path), "/sys/class/usb_role/%s-role-switch/role",
			       data->role_switch_name);
		if (len > 0 && (size_t)len < sizeof(path) && path_is_openable(path))
			return true;
	}

	if (iphone_dev_rockchip_phy_mode_path(data, path, sizeof(path)) && path_is_openable(path))
		return true;

	return false;
}

static int write_sysfs_string(const char *path, const char *value)
{
	struct file *file;
	loff_t pos = 0;
	size_t len = strlen(value);
	ssize_t written;

	file = filp_open(path, O_WRONLY, 0);
	if (IS_ERR(file))
		return PTR_ERR(file);

	written = kernel_write(file, value, len, &pos);
	filp_close(file, NULL);
	if (written < 0)
		return written;
	if ((size_t)written != len)
		return -EIO;

	return 0;
}

static int set_usb_role_sysfs(struct iphone_dev_data *data, enum usb_role role)
{
	char role_path[256];
	const char *role_str;
	int len;
	int ret;

	switch (role) {
	case USB_ROLE_HOST:
		role_str = "host";
		break;
	case USB_ROLE_DEVICE:
		role_str = "device";
		break;
	default:
		return -EINVAL;
	}

	if (iphone_dev_update_role_switch_name_from_gadget(data) &&
	    (!data->role_switch_name[0]))
		return -ENODEV;

	len = snprintf(role_path, sizeof(role_path),
		       "/sys/class/usb_role/%s-role-switch/role",
		       data->role_switch_name);
	if (len < 0 || (size_t)len >= sizeof(role_path))
		return -ENAMETOOLONG;

	ret = write_sysfs_string(role_path, role_str);
	if (ret)
		return ret;

	pr_info("iPhone: setting usb role via sysfs: %s -> %s\n",
		role_path, role_str);
	return 0;
}

/*
 * Rockchip rk3528: drive the role through the USB2 PHY.
 *
 * The PHY's otg_mode attribute forces the iddig line through the GRF, and the
 * SoC's OTG state machine reacts to that by switching the controller. Writing
 * the PHY first is what makes the change stick - see the comment on
 * iphone_dev_rockchip_phy_mode_path().
 */
static int set_usb_role_via_rockchip_phy(struct iphone_dev_data *data, enum usb_role role)
{
	const char *value = (role == USB_ROLE_HOST) ? "host" : "peripheral";
	char path[256];

	if (!iphone_dev_rockchip_phy_mode_path(data, path, sizeof(path)))
		return -ENODEV;

	return write_sysfs_string(path, value);
}

static int set_usb_role(struct iphone_dev_data *data, enum usb_role role)
{
	struct device *controller;
	int ret;

	if (!data)
		return -EINVAL;

	if (role != USB_ROLE_HOST && role != USB_ROLE_DEVICE)
		return -EINVAL;

	/*
	 * The fwnode lookup needs the controller, which is only reachable while
	 * the gadget is registered. It is resolved once and kept; the other
	 * mechanisms below do not need it at all, so do not fail here - the role
	 * switch unbinds the gadget before flipping, and cdev->gadget is NULL by
	 * then.
	 */
	if (!data->role_switch && data->cdev && data->cdev->gadget) {
		controller = data->cdev->gadget->dev.parent;
		if (!controller)
			controller = &data->cdev->gadget->dev;
		data->role_switch = usb_role_switch_find_by_fwnode(dev_fwnode(controller));
	}

	if (data->role_switch) {
		ret = usb_role_switch_set_role(data->role_switch, role);
		if (ret)
			return ret;
		if (usb_role_switch_get_role(data->role_switch) != role)
			return -EIO;

		pr_info("iPhone: USB role switched to %s\n", usb_role_string(role));
		return 0;
	}

	/* Boards that expose the usb_role class instead of a fwnode handle. */
	ret = set_usb_role_sysfs(data, role);
	if (!ret) {
		pr_info("iPhone: USB role switched to %s via usb_role class\n",
			usb_role_string(role));
		return 0;
	}
	if (ret != -ENODEV && ret != -ENOENT)
		return ret;

	/*
	 * Rockchip rk3528: its PHY otg_mode is what the OTG state machine listens
	 * to, and the controller follows it. Measured on a ROCK 2A - writing the
	 * controller's debugfs "mode" instead switches it and the state machine
	 * pulls it back within a second, and a kernel_write() to that debugfs file
	 * fails outright ("kernel write not supported"), so the PHY is the only
	 * mechanism that works there.
	 */
	ret = set_usb_role_via_rockchip_phy(data, role);
	if (!ret) {
		pr_info("iPhone: USB role switched to %s via Rockchip PHY otg_mode\n",
			usb_role_string(role));
		return 0;
	}

	pr_err("iPhone: no usable USB role switch for UDC '%s' (%d): set dr_mode = \"otg\" in the DT and provide either a usb_role class entry or a Rockchip USB2 PHY otg_mode attribute\n",
	       data->udc[0] ? data->udc : "<auto>", ret);
	return ret;
}


static int iphone_dev_set_otg_role_internal(struct iphone_dev_data *data,
					    enum usb_role role,
					    bool manage_gadget_lifecycle)
{
	int ret;
	bool role_is_device;
	bool role_already_set;

	if (!data || !data->driver)
		return -ENODEV;

	switch (role) {
	case USB_ROLE_DEVICE:
		role_is_device = true;
		break;
	case USB_ROLE_HOST:
		role_is_device = false;
		break;
	default:
		return -EINVAL;
	}

	role_already_set = data->otg_role_cache_valid &&
		data->otg_role_device_cached == role_is_device;

	if (role == USB_ROLE_HOST && manage_gadget_lifecycle) {
		ret = iphone_dev_unbind_gadget(data);
		if (ret)
			return ret;
	}

	if (!role_already_set) {
		ret = set_usb_role(data, role);
		if (ret) {
			pr_warn("iPhone: failed to switch USB role to %s: %d\n",
				usb_role_string(role), ret);
			return ret;
		}
		data->otg_role_device_cached = role_is_device;
		data->otg_role_cache_valid = true;

		pr_info("iPhone: set OTG role '%s'\n", usb_role_string(role));
	} else {
		/*
		 * Only the electrical role is cached.  Lifecycle work must still run:
		 * after a ROCK 2A device transition the UDC can appear about 120 ms
		 * late, so the first bind may fail even though the role itself stuck.
		 * Returning here used to make every later DEVICE retry a no-op and
		 * could leave the gadget permanently unpublished after a cable pull.
		 */
		pr_debug("iPhone: OTG role '%s' already cached; keeping lifecycle work\n",
			 usb_role_string(role));
	}

	if (role == USB_ROLE_DEVICE && manage_gadget_lifecycle) {
		ret = iphone_dev_bind_gadget(data);
		if (ret)
			return ret;
	}

	return 0;
}

static void iphone_dev_role_switch_work(struct work_struct *work)
{
	struct iphone_dev_data *data =
		container_of(work, struct iphone_dev_data, role_switch_work);
	struct iap2_acc_accessory *acc;

	if (g_iphone_get_status(&data->g) != RoleSwitch) {
		data->role_switch_work_scheduled = false;
		return;
	}

	if (data->role_switch_rebind_scheduled) {
		cancel_delayed_work_sync(&data->role_switch_rebind_work);
		data->role_switch_rebind_scheduled = false;
	}

	/* Complete the EP0 status stage before tearing down the UDC. */
	msleep(IPHONE_ROLE_SWITCH_HOST_DELAY_MS);

	if (iphone_dev_set_otg_role_internal(data, USB_ROLE_HOST, true)) {
		pr_warn("iPhone: role-switch host transition failed\n");
		if (iphone_dev_bind_gadget(data))
			pr_err("iPhone: failed to restore device gadget after role-switch failure\n");
		data->g.role_switch_requested = false;
		g_iphone_set_status(&data->g, Initial);
		data->role_switch_work_scheduled = false;
		return;
	}

	acc = iap2_acc_probe_accessory();
	if (IS_ERR(acc)) {
		pr_warn("iPhone: role-switch accessory probe failed: %ld\n",
			PTR_ERR(acc));
		/* Go back from host to device, but don't instantly publish the gadget yet; that will happen after debounce */
		/*ret = iphone_dev_set_otg_role_internal(data, USB_ROLE_DEVICE, false);
		if (ret) {
			pr_warn("iPhone: role-switch device rollback failed: %d\n",
				ret);
			data->g.role_switch_requested = false;
			g_iphone_set_status(&data->g, Initial);
			data->role_switch_work_scheduled = false;
			return;
		}*/

		data->g.role_switch_requested = false;
		g_iphone_set_status(&data->g, RoleSwitchFailed);
		data->role_switch_rebind_scheduled = true;
		mod_delayed_work(system_wq, &data->role_switch_rebind_work,
				 msecs_to_jiffies(IPHONE_ROLE_SWITCH_REBIND_DEBOUNCE_MS));
		data->role_switch_work_scheduled = false;
		return;
	}

	pr_info("iPhone: role-switch accessory probe succeeded for if=%s\n",
		acc->ifname);

	mutex_lock(&data->lock);
	iphone_dev_clear_accessory_locked(data);
	data->acc = acc;
	if (iphone_dev_add_accessory_link_locked(data, acc))
		pr_warn("iPhone: failed to create accessory iap2_accessory link\n");
	mutex_unlock(&data->lock);

	g_iphone_set_status(&data->g, Accessory);
	mod_delayed_work(system_wq, &data->accessory_watch_work,
			 msecs_to_jiffies(100));
	data->role_switch_work_scheduled = false;
}

static int iphone_dev_start_role_switch_probe(struct g_iphone *iphone_gadget)
{
	struct iphone_dev_data *data =
		container_of(iphone_gadget, struct iphone_dev_data, g);

	if (data->role_switch_work_scheduled)
		return 0;

	data->role_switch_work_scheduled = true;
	schedule_work(&data->role_switch_work);
	return 0;
}

static int iphone_dev_driver_bind(struct usb_composite_dev *cdev)
{
	struct iphone_dev_driver *ipdrv =
		container_of(cdev->driver, struct iphone_dev_driver, drv);
	struct iphone_dev_data *data = ipdrv->data;
	struct device *controller;
	data->cdev = cdev;
	data->gadget_registered = true;
	controller = cdev->gadget->dev.parent;
	if (!controller)
		controller = &cdev->gadget->dev;
	if (!data->role_switch)
		data->role_switch = usb_role_switch_find_by_fwnode(dev_fwnode(controller));

	/*
	 * Resolve the Rockchip PHY path now, while the controller is still
	 * reachable. The role switch unbinds the gadget before flipping, and
	 * cdev->gadget is NULL by then, so this is the only chance to find it.
	 */
	{
		char phy_path[256];

		(void)iphone_dev_rockchip_phy_mode_path(data, phy_path, sizeof(phy_path));
	}

	if (!data->role_switch) {
		/*
		 * No fwnode-backed switch. Accept a usb_role class entry named
		 * after the UDC, or a Rockchip USB2 PHY otg_mode attribute, instead;
		 * if neither exists the 0x51 handshake can never flip this port to
		 * host, so make that explicit rather than failing later with a
		 * confusing role-switch error.
		 */
		iphone_dev_update_role_switch_name_from_gadget(data);
		if (!iphone_dev_has_role_switch(data)) {
			pr_err("iPhone: no USB role switch for UDC '%s': set dr_mode = \"otg\" in the DT (which needs an extcon or usb-role-switch property), or provide a usb_role class entry / DT role-switch provider\n",
			       data->udc[0] ? data->udc : "<auto>");
			return -EPROBE_DEFER;
		}
		pr_info("iPhone: using usb_role sysfs fallback '%s'\n",
			data->role_switch_name);
	}

	pr_info("iPhone: driver binding 4 configurations\n");

	int ret;

	for (int i = 0; i < 4; i++)
	{
		ret = usb_add_config(cdev, &data->usb_configs[i].cfg, iphone_do_config);
		if (ret)
			return ret;
	}

	return 0;
}

static int iphone_dev_driver_unbind(struct usb_composite_dev *cdev)
{
	struct iphone_dev_driver *ipdrv =
		container_of(cdev->driver, struct iphone_dev_driver, drv);
	struct iphone_dev_data *data = ipdrv->data;

	pr_info("iPhone: driver unbind");

	data->role_switch_work_scheduled = false;
	mutex_lock(&data->lock);
	iphone_dev_clear_accessory_locked(data);
	mutex_unlock(&data->lock);
	data->cdev = NULL;
	data->gadget_registered = false;
	
	return 0;
}

struct iphone_dev_data *iphone_dev_alloc(struct device *owner_dev, char *udc_name, char* serial)
{
	int ret = -ENOMEM;
	struct iphone_dev_data *data;
	size_t count, i;

	if (!serial)
		serial = DEFAULT_IPHONE_SERIAL;

	data = kzalloc(sizeof(*data), GFP_KERNEL);
	if (!data)
		goto fail;

	/* copy device descriptor */
	data->dev_desc = iphone_device_desc;
	data->owner_dev = owner_dev;
	mutex_init(&data->lock);

	/* copy strings table */
	count = ARRAY_SIZE(iphone_strings);
	data->stringtab = kmemdup(iphone_strings,
	                           sizeof(iphone_strings),
	                           GFP_KERNEL);
	if (!data->stringtab)
		goto fail;

	for (i = 0; data->stringtab[i].s; i++) {
		if (data->stringtab[i].id == 3)
			data->stringtab[i].s = data->serial;
		else if (data->stringtab[i].id == 4)
			data->stringtab[i].s = data->serial_r;
	}

	/* copy serial and serial_r */
	strscpy(data->g.serial, serial, sizeof(data->g.serial));
	strscpy(data->serial, serial, sizeof(data->serial));
	snprintf(data->serial_r, sizeof(data->serial_r), "%s-R", serial);

	/* init gadget_strings */
	data->gadget_strings.language = 0x0409;
	data->gadget_strings.strings  = data->stringtab;

	/* init gadget_strings_array */
	data->gadget_strings_array[0] = &data->gadget_strings;
	data->gadget_strings_array[1] = NULL;

	/* init usb_composite_driver */
	data->driver = kzalloc(sizeof(*data->driver), GFP_KERNEL);
	if (!data->driver)
		goto fail;

	data->driver->drv = (struct usb_composite_driver) {
		.name       = data->driver_name,
		.dev        = &data->dev_desc,
		.strings    = data->gadget_strings_array,
		.max_speed  = USB_SPEED_SUPER,
		.bind       = iphone_dev_driver_bind,
		.unbind     = iphone_dev_driver_unbind,
		.udc_name   = data->udc_name_vec,
	};
	data->driver->data = data;
	data->udc_name_vec[0] = data->udc;
	data->udc_name_vec[1] = NULL;

	/* copy UDC name if provided */
	if (!udc_name) {
		data->driver->drv.udc_name = NULL;
		data->udc_auto = true;
	} else {
		const char *rs_name;

		strscpy(data->udc, udc_name, sizeof(data->udc));
		/* Lets the usb_role sysfs fallback work before cdev exists. */
		rs_name = iphone_dev_role_switch_name(udc_name);
		if (rs_name)
			strscpy(data->role_switch_name, rs_name,
				sizeof(data->role_switch_name));
	}

	/* create runtime-unique driver name */
	if (!udc_name) {
		snprintf(data->driver_name, sizeof(data->driver_name), "iphone");
	} else {
		snprintf(data->driver_name, sizeof(data->driver_name), "iphone-%s", udc_name);
	}

	/* init usb configs */
	for (int i = 0; i < ARRAY_SIZE(iphone_configs) && i < 4; i++) {
		data->usb_configs[i].g = &data->g;
		data->usb_configs[i].cfg = iphone_configs[i];
	}

	data->driver_registered = false;
	data->gadget_registered = false;
	data->otg_role_device_cached = true;
	data->otg_role_cache_valid = false;
	data->g.set_otg_role = iphone_dev_set_otg_role;
	data->g.start_role_switch_probe = iphone_dev_start_role_switch_probe;
	data->g.notify_status_changed = iphone_dev_notify_status_changed;
	INIT_WORK(&data->status_notify_work, iphone_dev_status_notify_workfn);
	INIT_WORK(&data->role_switch_work, iphone_dev_role_switch_work);
	INIT_DELAYED_WORK(&data->role_switch_rebind_work,
			  iphone_dev_role_switch_rebind_workfn);
	INIT_DELAYED_WORK(&data->accessory_watch_work,
			  iphone_dev_accessory_watch_work);

	pr_info("iPhone: initial gadget bind with udc_name '%s'\n",
		udc_name ? udc_name : "<auto>");

	ret = iphone_dev_bind_gadget(data);
	if (ret) {
		pr_warn("iPhone: initial gadget bind failed: %d\n", ret);
		goto fail;
	}

	pr_debug("iPhone: initial gadget bind complete\n");
	return data;

fail:
	if (data) {
		if (data->role_switch)
			usb_role_switch_put(data->role_switch);
		kfree(data->driver);
		kfree(data->stringtab);
		kfree(data);
	}
	return ERR_PTR(ret);
}

static void iphone_dev_stop_runtime(struct iphone_dev_data *data)
{
	if (!data)
		return;

	cancel_delayed_work_sync(&data->accessory_watch_work);
	cancel_delayed_work_sync(&data->role_switch_rebind_work);
	cancel_work_sync(&data->role_switch_work);
	cancel_work_sync(&data->status_notify_work);
	data->role_switch_work_scheduled = false;
	data->role_switch_rebind_scheduled = false;

	mutex_lock(&data->lock);
	iphone_dev_clear_accessory_locked(data);
	mutex_unlock(&data->lock);
}

int iphone_dev_free(struct iphone_dev_data *data) {
	if (!data || !data->driver) {
		return -ENODEV;
	}

	iphone_dev_stop_runtime(data);

	pr_debug("iPhone: calling gadget unregister\n");
	iphone_dev_unbind_gadget(data);
	cancel_work_sync(&data->status_notify_work);
	if (data->role_switch) {
		usb_role_switch_put(data->role_switch);
		data->role_switch = NULL;
	}
	
	/* This code has been checked several times to verify there is no potential for UAF */
	kfree(data->stringtab);
	kfree(data->driver);
	kfree(data);
	return 0;
}
