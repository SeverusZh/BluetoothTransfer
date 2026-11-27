using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class OppSendQueueTests
{
    private static OppSendJob Job(string name) => new() { DisplayName = name, SourcePath = name };

    [Fact]
    public async Task Queue_ProcessesInOrder()
    {
        var queue = new OppSendQueue();
        queue.Enqueue(new[] { Job("a"), Job("b"), Job("c") });

        var processed = new List<string>();
        await queue.StartAsync("00:11", (job, _) =>
        {
            processed.Add(job.DisplayName);
            return Task.FromResult(true);
        });

        Assert.Equal(new[] { "a", "b", "c" }, processed);
        Assert.Equal(0, queue.PendingCount);
        Assert.All(queue.Jobs, j => Assert.Equal(OppJobStatus.Ok, j.Status));
    }

    [Fact]
    public async Task Queue_FailedJob_CollectsError()
    {
        var queue = new OppSendQueue();
        queue.Enqueue(new[] { Job("bad") });

        await queue.StartAsync("00:11", (_, _) => Task.FromResult(false));

        var job = queue.Jobs.Single();
        Assert.Equal(OppJobStatus.Failed, job.Status);
        Assert.False(string.IsNullOrEmpty(job.Error));
    }

    [Fact]
    public async Task Queue_ProcessException_MarksFailed()
    {
        var queue = new OppSendQueue();
        queue.Enqueue(new[] { Job("boom") });

        await queue.StartAsync("00:11", (_, _) => throw new InvalidOperationException("连接失败"));

        var job = queue.Jobs.Single();
        Assert.Equal(OppJobStatus.Failed, job.Status);
        Assert.Contains("连接失败", job.Error);
    }

    [Fact]
    public async Task Queue_CancelPendingJobs()
    {
        var queue = new OppSendQueue();
        queue.Enqueue(new[] { Job("a"), Job("b"), Job("c") });
        queue.CancelAll();

        await queue.StartAsync("00:11", (_, _) => Task.FromResult(true));

        Assert.Equal(0, queue.PendingCount);
        Assert.Empty(queue.Jobs);
    }

    [Fact]
    public async Task Queue_CancelledProcessingJob_StopsAndCancelsRemaining()
    {
        var queue = new OppSendQueue();
        queue.Enqueue(new[] { Job("a"), Job("b"), Job("c") });

        using var cts = new CancellationTokenSource();
        var task = queue.StartAsync("00:11", (job, token) =>
        {
            if (job.DisplayName == "a")
            {
                cts.Cancel();
                token.ThrowIfCancellationRequested();
            }
            return Task.FromResult(true);
        }, cts.Token);
        await task;

        var first = queue.Jobs.First();
        Assert.Equal(OppJobStatus.Cancelled, first.Status);
        Assert.Single(queue.Jobs);
    }

    [Fact]
    public async Task Queue_Retry_FailedJobRequeued()
    {
        var queue = new OppSendQueue();
        var job = Job("retry-me");
        queue.Enqueue(new[] { job });

        await queue.StartAsync("00:11", (_, _) => Task.FromResult(false));
        Assert.Equal(OppJobStatus.Failed, job.Status);

        queue.Retry(job);
        Assert.Equal(OppJobStatus.Pending, job.Status);
        Assert.Equal(1, queue.PendingCount);

        await queue.StartAsync("00:11", (_, _) => Task.FromResult(true));
        Assert.Equal(OppJobStatus.Ok, job.Status);
    }

    /// <summary>
    /// 暂停后 Continue 的恢复语义：任务被第一次处理时暂停（取消当前尝试）再立即 Continue（改回 Pending），
    /// 首次尝试在已被取消的令牌下抛出 OCE，随后同一 StartAsync 循环重新取出该任务继续处理直至成功。
    /// 不再使用 Thread.Sleep 制造时序，而是通过 TCS 门控在 worker 尚未结算时执行 Pause/Continue。
    /// </summary>
    [Fact]
    public async Task Pause_Continue_ResumesJobInQueue()
    {
        var queue = new OppSendQueue();
        var job = new OppSendJob { Kind = "file", SourcePath = "a", DisplayName = "a", Size = 1 };
        queue.Enqueue(new[] { job });

        var processStarted = new TaskCompletionSource();
        var releaseFirst = new TaskCompletionSource();
        int processCalls = 0;
        int completed = 0;
        queue.JobCompleted += (j, ok) =>
        {
            if (ReferenceEquals(j, job)) System.Threading.Interlocked.Increment(ref completed);
        };

        var runTask = queue.StartAsync("AA:BB:CC:DD:EE:FF", (j, ct) =>
        {
            // 第一次进入发送：受控阻塞，待 Pause 取消令牌后放行并抛 OCE；恢复后的第二次调用直接成功。
            if (System.Threading.Interlocked.Increment(ref processCalls) == 1)
            {
                processStarted.TrySetResult();
                return AwaitThenThrow(releaseFirst.Task, ct);
            }
            return Task.FromResult(true);
        });

        await processStarted.Task;

        // worker 尚未结算时：先暂停（取消当前尝试），随即立即继续（改回 Pending）。
        queue.Pause(job);
        Assert.Equal(OppJobStatus.Paused, job.Status);
        queue.Continue(job);
        Assert.Equal(OppJobStatus.Pending, job.Status);

        // 放行第一次尝试：其在已取消令牌下抛出 OCE；任务保留在待处理队列，
        // 由 StartAsync 的下一次循环重新处理直至成功，验证“恢复后任务被重新处理至 Ok”。
        releaseFirst.TrySetResult();

        await runTask;

        Assert.Equal(OppJobStatus.Ok, job.Status);    // 恢复后最终被处理为成功
        Assert.Equal(2, processCalls);                // 首次尝试失败 + 恢复后重新处理一次
        Assert.Equal(1, completed);                   // 恰好触发一次 JobCompleted
        Assert.Equal(0, queue.PendingCount);          // 成功后任务已出队
    }

    [Fact]
    public void Remove_RemovesPendingJob()
    {
        var queue = new OppSendQueue();
        var job = new OppSendJob { Kind = "file", SourcePath = "a", DisplayName = "a", Size = 1 };
        queue.Enqueue(new[] { job });

        queue.Remove(job);

        Assert.DoesNotContain(job, queue.Jobs);
    }

    [Fact]
    public void PausePendingJob_MarksPaused_AndContinueMarksPending()
    {
        var queue = new OppSendQueue();
        var job = new OppSendJob { Kind = "file", SourcePath = "a", DisplayName = "a", Size = 1 };
        queue.Enqueue(new[] { job });

        queue.Pause(job);
        Assert.Equal(OppJobStatus.Paused, job.Status);

        queue.Continue(job);
        Assert.Equal(OppJobStatus.Pending, job.Status);
    }

    /// <summary>
    /// 回归：Pause 触发的取消尚未被 worker 结算时立即 Continue（把任务改回 Pending），
    /// 任务不得被误判为取消或移出待处理队列，而应由后续循环继续处理直至成功。
    /// 通过让 process 委托在取消时延后抛出的方式控制时序，不依赖固定 Sleep。
    /// </summary>
    [Fact]
    public async Task Pause_ThenImmediateContinue_JobNotLost()
    {
        var queue = new OppSendQueue();
        var job = new OppSendJob { Kind = "file", SourcePath = "a", DisplayName = "a", Size = 1 };
        queue.Enqueue(new[] { job });

        var processStarted = new TaskCompletionSource();
        var releaseProcess = new TaskCompletionSource();
        int processCalls = 0;
        int completed = 0;
        bool sawCancelled = false;

        queue.JobCompleted += (j, ok) =>
        {
            if (ReferenceEquals(j, job)) System.Threading.Interlocked.Increment(ref completed);
        };

        // 监听状态变化，以便显式断言任务从未被标为 Cancelled。
        System.ComponentModel.PropertyChangedEventHandler watcher = (s, e) =>
        {
            if (ReferenceEquals(s, job) && e.PropertyName == nameof(OppSendJob.Status)
                && job.Status == OppJobStatus.Cancelled)
                sawCancelled = true;
        };
        job.PropertyChanged += watcher;

        var runTask = queue.StartAsync("AA:BB:CC:DD:EE:FF", (j, ct) =>
        {
            // 第一次进入发送：受控阻塞，待测试放行后在已取消状态下抛出 OCE，模拟暂停取消；
            // 续传后的第二次调用直接成功。
            if (System.Threading.Interlocked.Increment(ref processCalls) == 1)
            {
                processStarted.TrySetResult();
                return AwaitThenThrow(releaseProcess.Task, ct);
            }
            return Task.FromResult(true);
        });

        try
        {
            await processStarted.Task;

            // 在 worker 尚未结算时：先暂停（取消当前尝试），随即立即继续（改回 Pending）。
            queue.Pause(job);
            queue.Continue(job);

            // 放行第一次调用：其观察到取消后抛出 OperationCanceledException。
            // 此时任务已是 Pending，旧实现的第二 catch 会误判为取消并从 _pending 移除，
            // 从而触发 finally 的 JobCompleted 导致任务丢失；本测试断言该竞态已被修复。
            releaseProcess.TrySetResult();

            await runTask;

            Assert.Equal(OppJobStatus.Ok, job.Status);  // 最终被处理为成功
            Assert.Equal(1, completed);                 // JobCompleted 恰好触发一次
            Assert.False(sawCancelled);                 // 任务从未被标为 Cancelled
        }
        finally
        {
            job.PropertyChanged -= watcher;
        }
    }

    /// <summary>受控等待：先等待测试放行的门控，再检查取消令牌并抛出（若已取消）。</summary>
    private static async Task<bool> AwaitThenThrow(Task gate, CancellationToken ct)
    {
        await gate;                 // 不在此处感知取消，避免提前抛出而无法控制时序
        ct.ThrowIfCancellationRequested();
        return true;
    }
}
