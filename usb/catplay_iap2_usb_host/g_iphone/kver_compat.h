// SPDX-License-Identifier: GPL-2.0
#pragma once

/*
 * Kernel-version shims for this out-of-tree gadget module.
 *
 * g_iphone is shared between the Ingenic/V821 trees, which track a newer
 * mainline, and the Rockchip 6.1 vendor kernel used by the ROCK 2A port. Only
 * the handful of APIs that changed in between are adapted here, so the module
 * keeps building on both.
 */

#include <linux/version.h>
#include <linux/device.h>
#include <linux/module.h>

/*
 * class_create() dropped its owner argument in 6.4, and the class and
 * class_attribute arguments of class attribute callbacks became const in the
 * same release. The Rockchip 6.1 vendor kernel still expects the older forms.
 */
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 4, 0)
#define IPHONE_CLASS_CREATE(name) class_create(name)
#define IPHONE_CLASS_ARG const struct class *
#define IPHONE_CLASS_ATTR_ARG const struct class_attribute *
#else
#define IPHONE_CLASS_CREATE(name) class_create(THIS_MODULE, name)
#define IPHONE_CLASS_ARG struct class *
#define IPHONE_CLASS_ATTR_ARG struct class_attribute *
#endif

/*
 * The unaligned accessors moved from asm/ to linux/ in 6.2.
 */
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 2, 0)
#include <linux/unaligned.h>
#else
#include <asm/unaligned.h>
#endif
