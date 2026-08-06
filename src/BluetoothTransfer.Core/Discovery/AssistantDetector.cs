using BluetoothTransfer.Core.Protocol;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Rfcomm;

namespace BluetoothTransfer.Core.Discovery;

/// <summary>
/// SDP 探测接收端是否运行助手（自定义 UUID）。
/// 输入可以是 OPP 服务条目 Id（Bluetooth#…#RFCOMM:…）或蓝牙设备 Id。
/// </summary>
public static class AssistantDetector
{
    public static async Task<RfcommDeviceService?> GetAssistantServiceAsync(
        string deviceId, CancellationToken ct = default)
    {
        if (string.IsNullOrWhiteSpace(deviceId)) return null;
        var baseId = deviceId;
        var idx = baseId.IndexOf("#RFCOMM", StringComparison.OrdinalIgnoreCase);
        if (idx > 0) baseId = baseId[..idx];

        BluetoothDevice? device = null;
        try
        {
            device = await BluetoothDevice.FromIdAsync(baseId).AsTask(ct);
            if (device == null) return null;
            var result = await device.GetRfcommServicesAsync(BluetoothCacheMode.Uncached).AsTask(ct);
            foreach (var svc in result.Services)
            {
                if (svc.ServiceId.Uuid != AsstConst.ServiceUuid) continue;
                var serviceId = baseId + "#RFCOMM:00000000:{" + AsstConst.ServiceUuid + "}";
                return await RfcommDeviceService.FromIdAsync(serviceId);
            }
            return null;
        }
        catch
        {
            return null;
        }
        finally
        {
            device?.Dispose();
        }
    }

    public static async Task<bool> DeviceHasAssistantAsync(string deviceId, CancellationToken ct = default)
    {
        var service = await GetAssistantServiceAsync(deviceId, ct);
        service?.Dispose();
        return service != null;
    }
}
