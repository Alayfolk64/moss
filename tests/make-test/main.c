#include <stdio.h>

#include "add.h"
#include "mul.h"

int main(void)
{
    int result = add(mul(5, 8), 2);

    printf("result = %d\n", result);
    return result == 42 ? 0 : 1;
}
