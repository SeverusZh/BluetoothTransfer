using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;

namespace BluetoothTransfer.Services;

public enum OppJobStatus
{
    Pending,
    Sending,
    Paused,
    Ok,
    Failed,
    Cancelled
}

/// <summary>发送队列中的单个任务（文件/文本/文件夹）。</summary>
public sealed class OppSendJob : INotifyPropertyChanged
{
    public string Id { get; } = Guid.NewGuid().ToString("N");
    public string Kind { get; init; } = "file"; // file / text / folder
    public string SourcePath { get; init; } = "";
    public string DisplayName { get; init; } = "";
    public long Size { get; init; }
    /// <summary>通道：opp（OPP 通用推送）或 assistant（接收助手私有协议）。</summary>
    public string Channel { get; set; } = "opp";
    public string ChannelDisplay => Channel == "assistant" ? "助手" : "OPP";

    private OppJobStatus _status = OppJobStatus.Pending;
    public OppJobStatus Status
    {
        get => _status;
        set
        {
            if (_status == value) return;
            _status = value;
            OnPropertyChanged(nameof(Status));
            OnPropertyChanged(nameof(StatusDisplay));
        }
    }

    private string _error = "";
    public string Error
    {
        get => _error;
        set
        {
            if (_error == value) return;
            _error = value;
            OnPropertyChanged(nameof(Error));
        }
    }

    public int Attempts { get; set; }
    public string StatusDisplay => Status switch
    {
        OppJobStatus.Pending => "等待中",
        OppJobStatus.Sending => "发送中",
        OppJobStatus.Paused => "已暂停",
        OppJobStatus.Ok => "成功",
        OppJobStatus.Failed => "失败",
        OppJobStatus.Cancelled => "已取消",
        _ => Status.ToString()
    };

    public event PropertyChangedEventHandler? PropertyChanged;

    private void OnPropertyChanged([CallerMemberName] string? name = null)
        => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}

/// <summary>
/// 发送任务队列：按添加顺序对同一设备顺序推送（Android OPP 同时只接受一个传输），
/// 支持取消全部、单任务重发。GUI 与 CLI 共用。
/// </summary>
public sealed class OppSendQueue
{
    private readonly object _lock = new();
    private readonly List<OppSendJob> _pending = new();
    private bool _running;
    private CancellationTokenSource? _cts;
    private CancellationTokenSource? _activeCts;
    private OppSendJob? _activeJob;

    public ObservableCollection<OppSendJob> Jobs { get; } = new();

    /// <summary>队列处理事件（任务完成时触发，携带任务与设备地址）。</summary>
    public event Action<OppSendJob, bool>? JobCompleted;

    public void Enqueue(IEnumerable<OppSendJob> jobs)
    {
        lock (_lock)
        {
            foreach (var job in jobs)
            {
                Jobs.Add(job);
                _pending.Add(job);
            }
        }
    }

    public int PendingCount
    {
        get
        {
            lock (_lock) return _pending.Count;
        }
    }

    public bool IsRunning
    {
        get
        {
            lock (_lock) return _running;
        }
    }

    /// <summary>暂停：暂停中的任务标记 Paused；发送中的任务取消当前尝试（接收端保留半成品，继续时续传）。</summary>
    public void Pause(OppSendJob job)
    {
        lock (_lock)
        {
            if (ReferenceEquals(job, _activeJob))
            {
                job.Status = OppJobStatus.Paused;
                job.Error = "";
                _activeCts?.Cancel();
            }
            else if (job.Status == OppJobStatus.Pending)
            {
                job.Status = OppJobStatus.Paused;
            }
        }
    }

    /// <summary>继续：把 Paused 任务放回待处理。</summary>
    public void Continue(OppSendJob job)
    {
        lock (_lock)
        {
            if (job.Status != OppJobStatus.Paused) return;
            job.Status = OppJobStatus.Pending;
            job.Error = "";
        }
    }

    /// <summary>移除任务：发送中则取消；从队列与待处理列表移除。</summary>
    public void Remove(OppSendJob job)
    {
        lock (_lock)
        {
            if (ReferenceEquals(job, _activeJob))
                _activeCts?.Cancel();
            Jobs.Remove(job);
            _pending.Remove(job);
        }
    }

    /// <summary>开始顺序处理队列；处理函数返回 true 表示成功。</summary>
    public async Task StartAsync(string deviceAddr, Func<OppSendJob, CancellationToken, Task<bool>> process, CancellationToken outerCt = default)
    {
        lock (_lock)
        {
            if (_running) return;
            _running = true;
            _cts = CancellationTokenSource.CreateLinkedTokenSource(outerCt);
        }
        try
        {
            while (true)
            {
                if (_cts!.IsCancellationRequested)
                {
                    lock (_lock)
                    {
                        foreach (var j in _pending.Where(j => j.Status is OppJobStatus.Pending or OppJobStatus.Paused).ToList())
                        {
                            j.Status = OppJobStatus.Cancelled;
                            j.Error = "用户取消";
                            Jobs.Remove(j);
                            _pending.Remove(j);
                        }
                    }
                    break;
                }
                OppSendJob? job;
                lock (_lock)
                {
                    job = _pending.FirstOrDefault(j => j.Status == OppJobStatus.Pending);
                }
                if (job == null) break;

                job.Status = OppJobStatus.Sending;
                CancellationTokenSource? activeCts = null;
                try
                {
                    lock (_lock)
                    {
                        _activeJob = job;
                        activeCts = _activeCts = CancellationTokenSource.CreateLinkedTokenSource(_cts.Token);
                    }
                    var ok = await process(job, activeCts.Token);
                    if (job.Status == OppJobStatus.Paused) continue;
                    job.Status = ok ? OppJobStatus.Ok : OppJobStatus.Failed;
                    if (!ok && string.IsNullOrEmpty(job.Error))
                        job.Error = "推送失败";
                }
                catch (OperationCanceledException) when (_cts.IsCancellationRequested)
                {
                    job.Status = OppJobStatus.Cancelled;
                    job.Error = "用户取消";
                }
                catch (OperationCanceledException)
                {
                    // 仅 Pause/Remove 触发（非取消全部）：Paused 保留，其余按取消处理
                    if (job.Status != OppJobStatus.Paused)
                    {
                        job.Status = OppJobStatus.Cancelled;
                        job.Error = "用户取消";
                    }
                }
                catch (Exception ex)
                {
                    job.Status = OppJobStatus.Failed;
                    job.Error = ex.Message;
                }
                finally
                {
                    lock (_lock)
                    {
                        _activeJob = null;
                        _activeCts = null;
                        activeCts?.Dispose();
                        if (job.Status != OppJobStatus.Paused)
                            _pending.Remove(job);
                    }
                    if (job.Status != OppJobStatus.Paused)
                        JobCompleted?.Invoke(job, job.Status == OppJobStatus.Ok);
                }
            }
        }
        finally
        {
            lock (_lock)
            {
                _running = false;
                _cts?.Dispose();
                _cts = null;
            }
        }
    }

    /// <summary>取消全部待处理任务（发送中的任务由 process 内的 CancellationToken 感知）。</summary>
    public void CancelAll()
    {
        lock (_lock)
        {
            _cts?.Cancel();
            foreach (var job in _pending.Where(j => j.Status is OppJobStatus.Pending or OppJobStatus.Paused).ToList())
            {
                job.Status = OppJobStatus.Cancelled;
                job.Error = "用户取消";
                Jobs.Remove(job);
                _pending.Remove(job);
            }
        }
    }

    /// <summary>把失败任务重新加入队尾。</summary>
    public void Retry(OppSendJob job)
    {
        if (job.Status != OppJobStatus.Failed) return;
        job.Status = OppJobStatus.Pending;
        job.Error = "";
        lock (_lock) _pending.Add(job);
    }
}
