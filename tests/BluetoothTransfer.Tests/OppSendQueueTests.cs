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
}
